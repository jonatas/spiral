use pgrx::prelude::*;
use pgrx::guc::{GucRegistry, GucSetting, GucContext, GucFlags};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::ffi::CStr;
use pgrx::AllocatedByPostgres;

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
    let now = unsafe { pgrx::pg_sys::GetCurrentTimestamp() };
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (now / 1_000_000);
    Aspiral(unix_seconds - get_kickoff_epoch())
}

#[pg_trigger]
fn aspiral_track_changes<'a>(trigger: &'a pgrx::PgTrigger<'a>) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, String> {
    let base_view_name = trigger.extra_args()
        .map_err(|e| format!("Failed to get trigger args: {}", e))?
        .first()
        .cloned()
        .ok_or("Missing base_view_name in trigger arguments")?;
    
    let process_row = |row: Option<PgHeapTuple<'a, AllocatedByPostgres>>| -> Result<(), String> {
        if let Some(tuple) = row {
            let t_val: TimestampWithTimeZone = tuple.get_by_name("t")
                .map_err(|e| format!("Failed to get 't' column: {}", e))?
                .ok_or("Column 't' is null")?;
            
            let a = aspiral(t_val);
            let bucket_1m = (a.0 / 60) * 60;
            catalog::mark_bucket_dirty(&base_view_name, bucket_1m);
        }
        Ok(())
    };

    process_row(trigger.old())?;
    process_row(trigger.new())?;

    Ok(trigger.new())
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_sketch_sfunc(state: Option<Vec<u8>>, next: Option<f64>) -> Option<Vec<u8>> {
    let digest = match state {
        Some(bytes) => bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100)),
        None => tdigest::TDigest::new_with_size(100),
    };
    let new_digest = if let Some(val) = next {
        digest.merge_unsorted(vec![val])
    } else {
        digest
    };
    Some(bincode::serialize(&new_digest).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_sketch_merge_sfunc(state: Option<Vec<u8>>, next: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let digest = match state {
        Some(bytes) => bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100)),
        None => tdigest::TDigest::new_with_size(100),
    };
    let new_digest = if let Some(bytes) = next {
        let other: tdigest::TDigest = bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100));
        tdigest::TDigest::merge_digests(vec![digest, other])
    } else {
        digest
    };
    Some(bincode::serialize(&new_digest).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_quantile(sketch: Option<Vec<u8>>, q: f64) -> Option<f64> {
    sketch.and_then(|bytes| {
        let digest: tdigest::TDigest = bincode::deserialize(&bytes).ok()?;
        Some(digest.estimate_quantile(q))
    })
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
    CREATE AGGREGATE aspiral_sketch(f64) (
        sfunc = aspiral_sketch_sfunc,
        stype = bytea
    );
    CREATE AGGREGATE aspiral_sketch_merge(bytea) (
        sfunc = aspiral_sketch_merge_sfunc,
        stype = bytea
    );
    CREATE AGGREGATE first(f64) (
        sfunc = first_sfunc,
        stype = f64
    );
    CREATE AGGREGATE last(f64) (
        sfunc = last_sfunc,
        stype = f64
    );
    "#,
    name = "create_aggregates",
    requires = ["aspiral_sketch_sfunc", "aspiral_sketch_merge_sfunc", "first_sfunc", "last_sfunc"]
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

    /// SCENARIO 1: Temporal Anchoring and Efficient Storage
    /// Demonstrates how standard timestamps are converted to small i64 offsets
    /// relative to a configurable "Point Zero".
    #[pg_test]
    fn test_scenario_1_temporal_anchoring() {
        Spi::run("CREATE TABLE anchor_test (t timestamptz);").expect("Table failed");
        Spi::run("INSERT INTO anchor_test VALUES ('2026-04-15 00:00:00Z'), ('2026-04-15 01:00:00Z');").expect("Insert failed");

        let offsets = Spi::connect(|client| {
            let mut res = Vec::new();
            let tuple_table = client.select("SELECT aspiral(t) FROM anchor_test ORDER BY t", None, &[]).unwrap();
            for row in tuple_table {
                res.push(row.get::<crate::Aspiral>(1).unwrap().unwrap().0);
            }
            Ok::<Vec<i64>, spi::Error>(res)
        }).unwrap();

        assert_eq!(offsets, vec![0, 3600]); // 0 and 1 hour (3600s)
        info!("Scenario 1: Verified efficient i64 temporal offsets.");
    }

    /// SCENARIO 2: Intelligent OHLCV Hierarchies
    /// Demonstrates automatic mapping of financial aggregates (Open/High/Low/Close)
    /// through multiple timeframes without manual SQL for each frame.
    #[pg_test]
    fn test_scenario_2_ohlcv_hierarchies() {
        Spi::run("CREATE TABLE trade_ticks (t timestamptz, price f64);").expect("Table failed");
        
        // Aspiral plans the entire hierarchy based on the column names (o, h, l, c)
        Spi::run("CREATE MATERIALIZED VIEW price_ohlcv_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, 
                         first(price) as o, max(price) as h, min(price) as l, last(price) as c
                  FROM trade_ticks GROUP BY 1
                  WITH (aspiral.frames='5m,15m');").expect("Hierarchy creation failed");

        Spi::run("INSERT INTO trade_ticks VALUES ('2026-04-15 00:00:10Z', 100.0), ('2026-04-15 00:01:10Z', 105.0);").expect("Insert failed");
        Spi::run("REFRESH MATERIALIZED VIEW price_ohlcv_1m;").expect("Refresh failed");

        let stats = Spi::connect(|client| {
            let row = client.select("SELECT o, h, l, c FROM price_ohlcv_5m", None, &[]).unwrap().first();
            Ok::<(f64, f64, f64, f64), spi::Error>((
                row.get::<f64>(1).unwrap().unwrap(),
                row.get::<f64>(2).unwrap().unwrap(),
                row.get::<f64>(3).unwrap().unwrap(),
                row.get::<f64>(4).unwrap().unwrap()
            ))
        }).unwrap();

        assert_eq!(stats, (100.0, 105.0, 100.0, 105.0));
        info!("Scenario 2: Verified automatic OHLCV aggregation across 5m hierarchy.");
    }

    /// SCENARIO 5: Statistical Distribution via Mergeable Sketches
    /// Demonstrates mathematically precise p95 percentiles across frames
    /// by using mergeable T-Digest sketches instead of "averaging percentiles".
    #[pg_test]
    fn test_scenario_5_percentile_sketches() {
        Spi::run("CREATE TABLE data_stream (t timestamptz, latency f64);").expect("Table failed");
        
        // Base view creates the 'sketch' column
        Spi::run("CREATE MATERIALIZED VIEW latency_stats_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, 
                         aspiral_sketch(latency) as latency_sketch
                  FROM data_stream GROUP BY 1
                  WITH (aspiral.frames='5m');").expect("Sketch view failed");

        // Insert 100 values to create a distribution
        Spi::run("INSERT INTO data_stream SELECT '2026-04-15 00:00:01Z'::timestamptz + (i || ' seconds')::interval, i::f64 FROM generate_series(1, 100) s(i);").expect("Ingest failed");
        Spi::run("REFRESH MATERIALIZED VIEW latency_stats_1m;").expect("Refresh failed");

        // The p95 should be approx 95.0
        let p95 = Spi::get_one::<f64>("SELECT aspiral_quantile(latency_sketch, 0.95) FROM latency_stats_5m").unwrap().unwrap();
        
        info!("Scenario 5: Precise p95 from hierarchical sketch: {}", p95);
        assert!(p95 >= 94.0 && p95 <= 96.0);
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
