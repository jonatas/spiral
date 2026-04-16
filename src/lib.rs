use pgrx::prelude::*;
use pgrx::guc::{GucRegistry, GucSetting, GucContext, GucFlags};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::ffi::CStr;

mod hooks;
mod catalog;
mod rollup;

::pgrx::pg_module_magic!(name, version);

static KICKOFF_DATE: GucSetting<Option<std::ffi::CString>> = GucSetting::<Option<std::ffi::CString>>::new(None);

#[no_mangle]
pub unsafe extern "C" fn _PG_init() {
    hooks::init_hooks();

    GucRegistry::define_string_guc(
        CStr::from_bytes_with_nul(b"aspiral.kickoff_date\0").unwrap(),
        CStr::from_bytes_with_nul(b"Point zero for the aspiral timeline (YYYY-MM-DD)\0").unwrap(),
        CStr::from_bytes_with_nul(b"The first day the system starts operating as the point zero.\0").unwrap(),
        &KICKOFF_DATE,
        GucContext::Suset,
        GucFlags::default(),
    );
}

fn get_kickoff_epoch() -> i64 {
    1776211200 // 2026-04-15
}

#[derive(PostgresType, Serialize, Deserialize, PostgresEq, PostgresOrd, PostgresHash, Copy, Clone, Debug, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct Aspiral(i64);

impl Deref for Aspiral {
    type Target = i64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral(t: TimestampWithTimeZone) -> Aspiral {
    let micros_since_pg_epoch = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (micros_since_pg_epoch / 1_000_000);
    let offset = unix_seconds - get_kickoff_epoch();
    Aspiral(offset)
}

#[pg_extern(immutable, parallel_safe)]
fn to_timestamptz(a: Aspiral) -> TimestampWithTimeZone {
    let unix_seconds = a.0 + get_kickoff_epoch();
    let pg_epoch_unix: i64 = 946684800;
    let micros_since_pg_epoch = (unix_seconds - pg_epoch_unix) * 1_000_000;
    unsafe { TimestampWithTimeZone::from_datum(pg_sys::Datum::from(micros_since_pg_epoch), false).unwrap() }
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use pgrx::datum::DatumWithOid;

    #[pg_test]
    fn test_aspiral_hierarchical_avg_precision() {
        Spi::run("CREATE TABLE stock_ticks (t timestamptz, price decimal);").expect("Table failed");
        
        Spi::run("CREATE MATERIALIZED VIEW ohlcv_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, 
                         sum(price) as price_sum, 
                         count(price) as price_count, 
                         max(price) as price_max
                  FROM stock_ticks GROUP BY 1
                  WITH (aspiral.frames='5m,15m');").expect("Base view failed");

        Spi::run("INSERT INTO stock_ticks VALUES ('2026-04-15 00:00:10Z', 100.0), ('2026-04-15 00:00:40Z', 110.0);").expect("Insert min 0");
        Spi::run("INSERT INTO stock_ticks VALUES ('2026-04-15 00:01:20Z', 120.0);").expect("Insert min 1");
        
        info!("--- Refreshing ohlcv_1m ---");
        Spi::run("REFRESH MATERIALIZED VIEW ohlcv_1m;").expect("Refresh failed");
        
        let stats = Spi::connect(|client| {
            // pgrx 0.17 client.select takes &[DatumWithOid] directly (no Some/Option)
            let row = client.select("SELECT sum(price_sum), sum(price_count) FROM ohlcv_1m_5m", None, &[]).unwrap().first();
            let sum = row.get::<f64>(1).unwrap().unwrap();
            let count = row.get::<i64>(2).unwrap().unwrap();
            Ok::<(f64, i64), spi::Error>((sum, count))
        }).unwrap();
        
        info!("Hierarchical Stats (5m): Sum={}, Count={}", stats.0, stats.1);
        assert_eq!(stats.0, 330.0);
        assert_eq!(stats.1, 3);
        assert_eq!(stats.0 / stats.1 as f64, 110.0);
    }
}

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries = 'aspiral'"]
    }
}
