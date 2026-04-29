use pgrx::bgworkers::*;
use pgrx::pg_sys;
use pgrx::prelude::*;
use std::ffi::CStr;

#[pg_guard]
#[no_mangle]
pub unsafe extern "C-unwind" fn spiral_worker_main(arg: pg_sys::Datum) {
    let db_oid = pg_sys::Oid::from_datum(arg, false).expect("Invalid DB OID");
    let dbname_ptr = pg_sys::get_database_name(db_oid);
    if dbname_ptr.is_null() {
        return;
    }
    let dbname = CStr::from_ptr(dbname_ptr).to_string_lossy().into_owned();

    BackgroundWorker::connect_worker_to_spi(Some(&dbname), None);

    // Use a specific advisory lock ID for Spiral workers (0x41535049 = 'ASPI')
    let lock_id: i64 = 0x41535049;
    let already_running: bool = Spi::get_one_with_args::<bool>(
        "SELECT NOT pg_try_advisory_lock($1)",
        &[Some(lock_id.into_datum()).into()],
    )
    .unwrap_or(Some(true))
    .unwrap_or(true);

    if already_running {
        debug2!(
            "Spiral Worker for database '{}' is already running. Exiting.",
            dbname
        );
        return;
    }

    info!("Spiral Background Worker Started on database '{}'.", dbname);

    while BackgroundWorker::wait_latch(Some(std::time::Duration::from_secs(60))) {
        let _ = Spi::connect(|client| {
            // Find all root materialized views (parent_view = 'BASE')
            let tuple_table = client.select(
                "SELECT view_name FROM spiral.metadata WHERE parent_view = 'BASE'",
                None,
                &[],
            )?;

            for row in tuple_table {
                if let Ok(Some(view_name)) = row.get::<String>(1) {
                    // Only refresh if there are dirty buckets
                    let has_dirty: bool = client
                        .select(
                            &format!("SELECT 1 FROM spiral.changelog WHERE base_view = $1 LIMIT 1"),
                            Some(1),
                            &[Some(view_name.clone().into_datum()).into()],
                        )
                        .and_then(|t| Ok(!t.is_empty()))
                        .unwrap_or(false);

                    if has_dirty {
                        info!("Spiral Worker: Auto-refreshing root view '{}'", view_name);
                        let _ = Spi::run(&format!("SELECT spiral_refresh('{}')", view_name));
                    }
                }
            }
            Ok::<(), spi::Error>(())
        });
    }
}

use std::cell::Cell;
thread_local! {
    static WORKER_STARTED: Cell<bool> = Cell::new(false);
}

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

    // Register a background worker
    // Note: pgrx's BackgroundWorkerBuilder::load() calls RegisterBackgroundWorker
    // which normally only works during _PG_init. For dynamic registration,
    // we might need to use the C API if pgrx doesn't expose it.
    // However, let's see if load() works or if there is another method.
    BackgroundWorkerBuilder::new(&format!("Spiral Worker: {}", dbname))
        .set_function("spiral_worker_main")
        .set_library("spiral")
        .set_argument(Some(db_oid.into_datum().expect("Failed to create datum")))
        .set_start_time(BgWorkerStartTime::PostmasterStart)
        .load();

    WORKER_STARTED.with(|f| f.set(true));
}
