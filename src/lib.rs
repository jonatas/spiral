use pgrx::prelude::*;
use pgrx::guc::{GucRegistry, GucSetting, GucContext, GucFlags};
use pgrx::datum::DatumWithOid;
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

#[pg_extern(immutable, parallel_safe)]
fn aspiral_now() -> Aspiral {
    let now = unsafe { pgrx::pg_sys::GetCurrentTimestamp() }; // Postgres epoch micros
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (now / 1_000_000);
    Aspiral(unix_seconds - get_kickoff_epoch())
}

#[pg_extern(immutable, parallel_safe)]
fn first_sfunc(state: Option<f64>, next: Option<f64>) -> Option<f64> {
    state.or(next)
}

#[pg_extern(immutable, parallel_safe)]
fn last_sfunc(_state: Option<f64>, next: Option<f64>) -> Option<f64> {
    next
}

extension_sql!(
    r#"
    CREATE AGGREGATE first(f64) (
        sfunc = first_sfunc,
        stype = f64
    );
    CREATE AGGREGATE last(f64) (
        sfunc = last_sfunc,
        stype = f64
    );
    "#,
    name = "create_first_last_aggregates",
    requires = ["first_sfunc", "last_sfunc"]
);

#[pg_extern(immutable, parallel_safe)]
fn aspiral_histogram(data: Vec<f64>, min: f64, max: f64, buckets: i32) -> pgrx::JsonB {
    let mut counts = vec![0i64; buckets as usize];
    let range = max - min;
    if range <= 0.0 {
        return pgrx::JsonB(serde_json::to_value(counts).unwrap());
    }

    for val in data {
        if val >= min && val <= max {
            let mut bucket = ((val - min) / range * buckets as f64) as usize;
            if bucket >= buckets as usize {
                bucket = buckets as usize - 1;
            }
            counts[bucket] += 1;
        }
    }
    pgrx::JsonB(serde_json::to_value(counts).unwrap())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_aspiral_ohlcv_and_histograms() {
        Spi::run("CREATE TABLE trades (t timestamptz, price f64, volume int);").expect("Table failed");
        
        Spi::run("CREATE MATERIALIZED VIEW candles_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, 
                         first(price) as o, 
                         max(price) as h, 
                         min(price) as l, 
                         last(price) as c,
                         sum(volume) as volume
                  FROM trades GROUP BY 1
                  WITH (aspiral.frames='5m,15m');").expect("Candles base view failed");

        Spi::run("INSERT INTO trades VALUES 
                  ('2026-04-15 00:00:10Z', 100.0, 10), 
                  ('2026-04-15 00:00:30Z', 105.0, 5),
                  ('2026-04-15 00:01:10Z', 102.0, 8),
                  ('2026-04-15 00:01:50Z', 110.0, 12);").expect("Insert trades failed");
        
        Spi::run("REFRESH MATERIALIZED VIEW candles_1m;").expect("Refresh candles failed");

        let stats = Spi::connect(|client| {
            let row = client.select("SELECT o, h, l, c, volume FROM candles_1m_5m", None, &[]).unwrap().first();
            let o = row.get::<f64>(1).unwrap().unwrap();
            let h = row.get::<f64>(2).unwrap().unwrap();
            let l = row.get::<f64>(3).unwrap().unwrap();
            let c = row.get::<f64>(4).unwrap().unwrap();
            let v = row.get::<i64>(5).unwrap().unwrap();
            Ok::<(f64, f64, f64, f64, i64), spi::Error>((o, h, l, c, v))
        }).unwrap();
        
        info!("5m Candle: O={}, H={}, L={}, C={}, V={}", stats.0, stats.1, stats.2, stats.3, stats.4);
        assert_eq!(stats.0, 100.0);
        assert_eq!(stats.1, 110.0);
        assert_eq!(stats.2, 100.0);
        assert_eq!(stats.3, 110.0);
        assert_eq!(stats.4, 35);

        let hist = Spi::connect(|client| {
            let val = client.select("SELECT aspiral_histogram(ARRAY[1.0, 2.0, 5.0, 8.0, 10.0], 0.0, 10.0, 2)", None, &[]).unwrap().first();
            let j = val.get::<pgrx::JsonB>(1).unwrap().unwrap();
            Ok::<pgrx::JsonB, spi::Error>(j)
        }).unwrap();
        info!("Histogram: {:?}", hist.0);
    }

    #[pg_test]
    fn test_aspiral_closed_frame_logic() {
        Spi::run("CREATE TABLE ticks_closed (t timestamptz, price f64);").expect("Table failed");
        
        Spi::run("CREATE MATERIALIZED VIEW base_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, 
                         max(price) as price_max
                  FROM ticks_closed GROUP BY 1
                  WITH (aspiral.frames='5m');").expect("Base view failed");

        Spi::run("INSERT INTO ticks_closed VALUES (now() - interval '1 hour', 50.0);").expect("Insert past failed");
        Spi::run("INSERT INTO ticks_closed VALUES (now(), 100.0);").expect("Insert now failed");

        Spi::run("REFRESH MATERIALIZED VIEW base_1m;").expect("Refresh base failed");

        let count = Spi::get_one::<i64>("SELECT count(*) FROM base_1m_5m").unwrap().unwrap();
        let max_p = Spi::get_one::<f64>("SELECT max(price_max) FROM base_1m_5m").unwrap().unwrap();
        
        info!("Closed frame check: Count={}, Max={}", count, max_p);
        assert_eq!(count, 1, "Should only have one bucket (the closed one)");
        assert_eq!(max_p, 50.0, "Should not have picked up the 100.0 price from the open bucket");
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
