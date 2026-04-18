use pgrx::prelude::*;
use pgrx::bgworkers::*;

#[pg_guard]
#[no_mangle]
pub unsafe extern "C-unwind" fn aspiral_worker_main(_arg: pg_sys::Datum) {
    // BackgroundWorker::attach_signal_handlers(SignalHandlerOptions::ConfigCustom);

    info!("Aspiral Background Worker Started.");

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
                    info!("Aspiral Worker: Auto-refreshing root view '{}'", view_name);
                    // Use unsafe Spi::run for simple command execution
                    unsafe {
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
