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
        for i in 1..=max_workers {
            let got_lock: bool = Spi::get_one::<bool>(&format!(
                "SELECT pg_try_advisory_lock({}, {})",
                db_oid_val.to_u32(),
                i
            ))
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

        let mut scopes_by_base: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (bv, sv) in scopes {
            scopes_by_base.entry(bv).or_default().push(sv);
        }

        let mut processed = 0i32;
        for (base_view, base_scopes) in scopes_by_base {
            if processed >= batch_size {
                break;
            }

            let num_processed = BackgroundWorker::transaction(|| {
                Spi::connect(|client| {
                    // 1. Check if jobs are paused for this DB (e.g. during DDL/testing)
                    let can_work: bool = client
                        .select(
                            &format!(
                                "SELECT pg_try_advisory_xact_lock_shared({}, 0)",
                                db_oid_val.to_u32()
                            ),
                            Some(1),
                            &[],
                        )?
                        .first()
                        .get_one::<bool>()
                        .unwrap_or(Some(false))
                        .unwrap_or(false);

                    if !can_work {
                        return Ok::<i32, spi::Error>(0);
                    }

                    // 2. Try to get the scope locks
                    let mut locked_scopes = Vec::new();
                    for scope_json in &base_scopes {
                        if processed + (locked_scopes.len() as i32) >= batch_size {
                            break;
                        }

                        let lock_key = format!("{}:{}", base_view, scope_json);
                        let lock_hash = crate::zorder::fnv1a_64(lock_key.as_bytes()) as i32;

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

                        if got_lock {
                            locked_scopes.push(scope_json.clone());
                        }
                    }

                    if locked_scopes.is_empty() {
                        return Ok::<i32, spi::Error>(0);
                    }

                    if debug_logging {
                        debug2!(
                            "Spiral Worker (Slot {}): refreshing {} scopes for '{}'",
                            slot,
                            locked_scopes.len(),
                            base_view
                        );
                    }

                    let safe_bv = base_view.replace('\'', "''");
                    let json_array =
                        serde_json::to_string(&locked_scopes).unwrap_or_else(|_| "[]".to_string());
                    let safe_json = json_array.replace('\'', "''");
                    let _ = Spi::run(&format!(
                        "SELECT spiral_refresh_scopes('{}', '{}'::jsonb)",
                        safe_bv, safe_json
                    ));
                    Ok::<i32, spi::Error>(locked_scopes.len() as i32)
                })
            })
            .unwrap_or(0);

            processed += num_processed;
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

    if !crate::WORKER_ENABLED.get() {
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
