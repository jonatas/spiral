use pgrx::bgworkers::*;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;

// Namespace for scope-level advisory locks (FNV of "spiral:scope").
const SCOPE_LOCK_NS: i32 = 0x5350_5343_u32 as i32;

#[pg_guard]
#[no_mangle]
pub unsafe extern "C-unwind" fn spiral_worker_main(arg: pg_sys::Datum) {
    let db_oid_val = pg_sys::Oid::from_datum(arg, false).expect("Invalid DB OID");

    BackgroundWorker::connect_worker_to_spi_by_oid(Some(db_oid_val), None);
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);

    let max_workers = crate::WORKER_MAX.get();

    let my_slot_id: Option<i32> = BackgroundWorker::transaction(|| {
        let mut slot = None;
        for i in 0..max_workers {
            let lock_id: i64 = 0x41535049 + i as i64;
            let got_lock: bool =
                Spi::get_one::<bool>(&format!("SELECT pg_try_advisory_lock({})", lock_id))
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

        let (scopes, debug_logging, batch_size) = BackgroundWorker::transaction(|| {
            Spi::connect(|client| {
                if !crate::WORKER_ENABLED.get() {
                    return Ok::<_, spi::Error>(None);
                }

                let table_exists = !client
                    .select(
                        "SELECT 1 FROM information_schema.tables \
                         WHERE table_schema = 'spiral' AND table_name = 'changelog' LIMIT 1",
                        Some(1),
                        &[],
                    )?
                    .is_empty();
                if !table_exists {
                    return Ok(None);
                }

                let debug_logging = crate::WORKER_DEBUG.get();
                let batch_size = crate::WORKER_BATCH_SIZE.get();

                // Scope-affinity scheduling:
                // Claim (base_view, scope_values) pairs ordered by oldest t_start first
                // (highest lag processed first).
                let scopes: Vec<(String, String)> = client
                    .select(
                        &format!(
                            "SELECT base_view, scope_values::text \
                             FROM spiral.changelog \
                             GROUP BY base_view, scope_values \
                             ORDER BY MIN(t_start) ASC \
                             LIMIT {}",
                            batch_size * 2 // fetch extras so workers can skip locked ones
                        ),
                        None,
                        &[],
                    )?
                    .map(|row| {
                        let bv = row.get::<String>(1).unwrap().unwrap_or_default();
                        let sv = row
                            .get::<String>(2)
                            .unwrap()
                            .unwrap_or_else(|| "{}".to_string());
                        (bv, sv)
                    })
                    .collect();

                Ok(Some((scopes, debug_logging, batch_size)))
            })
        })
        .unwrap_or(None)
        .unwrap_or((vec![], false, 10));

        let mut processed = 0i32;
        for (base_view, scope_json) in scopes {
            if processed >= batch_size {
                break;
            }

            let lock_key = format!("{}:{}", base_view, scope_json);
            let lock_hash = crate::zorder::fnv1a_64(lock_key.as_bytes()) as i32;

            let success = BackgroundWorker::transaction(|| {
                Spi::connect(|client| {
                    let got_lock: bool = client
                        .select(
                            &format!(
                                "SELECT pg_try_advisory_xact_lock({}, {})",
                                SCOPE_LOCK_NS, lock_hash
                            ),
                            Some(1),
                            &[],
                        )?
                        .first()
                        .get_one::<bool>()
                        .unwrap_or(Some(false))
                        .unwrap_or(false);

                    if !got_lock {
                        return Ok::<bool, spi::Error>(false);
                    }

                    if debug_logging {
                        debug2!(
                            "Spiral Worker (Slot {}): refreshing scope '{}' for '{}'",
                            slot,
                            scope_json,
                            base_view
                        );
                    }

                    let safe_bv = base_view.replace('\'', "''");
                    let safe_sv = scope_json.replace('\'', "''");
                    let _ = Spi::run(&format!(
                        "SELECT spiral_refresh_scope('{}', '{}')",
                        safe_bv, safe_sv
                    ));
                    Ok::<bool, spi::Error>(true)
                })
            })
            .unwrap_or(false);

            if success {
                processed += 1;
            }
        }

        if processed > 0 && !debug_logging {
            info!(
                "Spiral Worker (Slot {}): processed {} scope(s)",
                slot, processed
            );
        }
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
