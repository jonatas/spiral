use pgrx::prelude::*;
use pgrx::bgworkers::*;

#[pg_guard]
#[no_mangle]
pub unsafe extern "C-unwind" fn aspiral_worker_main(_arg: pg_sys::Datum) {
    let dbname = crate::BGWORKER_DBNAME.get().map(|s| s.to_string_lossy().into_owned());
    let dbname = dbname.as_deref().unwrap_or("aspiral");
    
    BackgroundWorker::connect_worker_to_spi(Some(dbname), None);

    info!("Aspiral Background Worker Started on database '{}'.", dbname);

    while BackgroundWorker::wait_latch(Some(std::time::Duration::from_secs(60))) {
        let _ = Spi::connect(|client| {
            // Find all root materialized views (parent_view = 'BASE')
            let tuple_table = client.select(
                "SELECT view_name FROM aspiral.metadata WHERE parent_view = 'BASE'",
                None,
                &[]
            )?;

            for row in tuple_table {
                if let Ok(Some(view_name)) = row.get::<String>(1) {
                    // Only refresh if there are dirty buckets
                    let has_dirty: bool = client.select(
                        &format!("SELECT 1 FROM aspiral.changelog WHERE base_view = $1 LIMIT 1"),
                        Some(1),
                        &[Some(view_name.clone().into_datum()).into()]
                    ).and_then(|t| Ok(!t.is_empty())).unwrap_or(false);

                    if has_dirty {
                        info!("Aspiral Worker: Auto-refreshing root view '{}'", view_name);
                        let _ = Spi::run(&format!("REFRESH MATERIALIZED VIEW {}", view_name));
                    }
                }
            }
            Ok::<(), spi::Error>(())
        });
    }
}

pub unsafe fn init_worker() {
    BackgroundWorkerBuilder::new("Aspiral Refresh Worker")
        .set_function("aspiral_worker_main")
        .set_library("aspiral")
        .set_start_time(BgWorkerStartTime::RecoveryFinished)
        .load();
}
