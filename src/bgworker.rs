use pgrx::bgworkers::*;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;

#[pg_guard]
#[no_mangle]
pub unsafe extern "C-unwind" fn spiral_worker_main(arg: pg_sys::Datum) {
    let db_oid_val = pg_sys::Oid::from_datum(arg, false).expect("Invalid DB OID");

    BackgroundWorker::connect_worker_to_spi_by_oid(Some(db_oid_val), None);

    let max_workers = crate::WORKER_MAX.get();

    let my_slot_id: Option<i32> = BackgroundWorker::transaction(|| {
        let mut slot = None;
        for i in 0..max_workers {
            let lock_id: i64 = 0x41535049 + i as i64;
            let got_lock: bool = Spi::get_one_with_args::<bool>(
                "SELECT pg_try_advisory_lock($1)",
                &[Some(lock_id.into_datum()).into()],
            )
            .unwrap_or(Some(false))
            .unwrap_or(false);

            if got_lock {
                slot = Some(i);
                break;
            }
        }

        if let Some(s) = slot {
            info!("Spiral Background Worker Started (Slot {}).", s);
        }
        slot
    });

    if my_slot_id.is_none() {
        debug2!("All Spiral Worker slots are filled. Exiting.");
        return;
    }

    let slot = my_slot_id.unwrap();

    while BackgroundWorker::wait_latch(Some(std::time::Duration::from_secs(1))) {
        unsafe {
            if std::ptr::read_volatile(std::ptr::addr_of!(pg_sys::ConfigReloadPending)) != 0 {
                std::ptr::write_volatile(std::ptr::addr_of_mut!(pg_sys::ConfigReloadPending), 0);
                pg_sys::ProcessConfigFile(pg_sys::GucContext::PGC_SIGHUP);
                info!("Spiral Worker (Slot {}): Configuration reloaded", slot);
            }
        }

        BackgroundWorker::transaction(|| {
            let _ = Spi::connect(|client| {
                let enabled = crate::WORKER_ENABLED.get();
                if !enabled {
                    return Ok::<(), spi::Error>(());
                }

                let debug_logging = crate::WORKER_DEBUG.get();

                // Find all root materialized views (parent_view = 'BASE')
                let tuple_table = client.select(
                    "SELECT view_name FROM spiral.metadata WHERE parent_view = 'BASE'",
                    None,
                    &[],
                )?;

                for row in tuple_table {
                    if let Ok(Some(view_name)) = row.get::<String>(1) {
                        // Avoid contention: try to acquire a transaction-level advisory lock for this specific view
                        // We use a constant namespace 0x41535050 = 1095983184 and the hashtext of the view name.
                        let got_view_lock: bool = client.select(
                            "SELECT pg_try_advisory_xact_lock(1095983184, hashtext($1))",
                            Some(1),
                            &[unsafe {
                                pgrx::datum::DatumWithOid::new(
                                    view_name.clone().into_datum().unwrap(),
                                    pg_sys::TEXTOID,
                                )
                            }],
                        )?.first().get_one::<bool>().unwrap_or(Some(false)).unwrap_or(false);

                        if !got_view_lock {
                            continue; // Another worker is processing this view
                        }

                        // Only refresh if there are dirty buckets
                        let has_dirty: bool = client
                            .select(
                                "SELECT 1 FROM spiral.changelog WHERE base_view = $1 LIMIT 1",
                                Some(1),
                                &[unsafe {
                                pgrx::datum::DatumWithOid::new(
                                    view_name.clone().into_datum().unwrap(),
                                    pg_sys::TEXTOID,
                                )
                            }],
                            )
                            .map(|t| !t.is_empty())
                            .unwrap_or(false);

                        if has_dirty {
                            if debug_logging {
                                debug2!("Spiral Worker (Slot {}): Auto-refreshing root view '{}'", slot, view_name);
                            } else {
                                info!("Spiral Worker (Slot {}): Auto-refreshing root view '{}'", slot, view_name);
                            }
                            let _ = Spi::run(&format!("SELECT spiral_refresh('{}')", view_name));
                        }
                    }
                }
                Ok::<(), spi::Error>(())
            });
        });
    }
}

use std::cell::Cell;
thread_local! {
    static WORKER_STARTED: Cell<bool> = const { Cell::new(false) };
}

/// # Safety
///
/// This function is unsafe because it interacts with PostgreSQL's background worker system.
pub unsafe fn maybe_start_worker() {
    if WORKER_STARTED.with(|f| f.get()) {
        return;
    }

    let db_oid = pg_sys::MyDatabaseId;
    let dbname_ptr = pg_sys::get_database_name(db_oid);
    if dbname_ptr.is_null() {
        return;
    }
    let dbname = CStr::from_ptr(dbname_ptr).to_string_lossy();

    let max_workers = crate::WORKER_MAX.get();

    for _ in 0..max_workers {
        if let Err(e) = BackgroundWorkerBuilder::new(&format!("Spiral Worker: {}", dbname))
            .set_function("spiral_worker_main")
            .set_library("spiral")
            .set_argument(Some(db_oid.into_datum().expect("Failed to create datum")))
            .enable_spi_access()
            .load_dynamic()
        {
            warning!("Failed to start dynamic background worker: {:?}", e);
        }
    }

    WORKER_STARTED.with(|f| f.set(true));
}
