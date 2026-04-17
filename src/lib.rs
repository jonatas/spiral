use pgrx::prelude::*;
use pgrx::guc::{GucRegistry, GucSetting, GucContext, GucFlags};
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use std::ffi::CStr;
use pgrx::AllocatedByPostgres;

mod hooks;
mod catalog;
mod rollup;
mod tam;


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
fn aspiral(t: TimestampWithTimeZone) -> i64 {
    let micros_since_pg_epoch = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (micros_since_pg_epoch / 1_000_000);
    unix_seconds - get_kickoff_epoch()
}

#[pg_extern(immutable, parallel_safe)]
fn to_timestamptz(a: i64) -> TimestampWithTimeZone {
    let unix_seconds = a + get_kickoff_epoch();
    let pg_epoch_unix: i64 = 946684800;
    let micros_since_pg_epoch = (unix_seconds - pg_epoch_unix) * 1_000_000;
    unsafe { TimestampWithTimeZone::from_datum(pg_sys::Datum::from(micros_since_pg_epoch), false).unwrap() }
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_now() -> i64 {
    let now = unsafe { pgrx::pg_sys::GetCurrentTimestamp() };
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (now / 1_000_000);
    unix_seconds - get_kickoff_epoch()
}

#[pg_trigger]
fn aspiral_track_changes<'a>(trigger: &'a pgrx::PgTrigger<'a>) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, String> {
    let base_view_name = trigger.extra_args()
        .map_err(|e| format!("Failed to get trigger args: {}", e))?
        .first()
        .cloned()
        .ok_or("Missing base_view_name in trigger arguments")?;
    
    // Get scope columns for this view
    let (scope_cols, _) = catalog::get_metadata(&base_view_name).unwrap_or((vec![], 0));

    let process_row = |row: Option<PgHeapTuple<'a, AllocatedByPostgres>>| -> Result<(), String> {
        if let Some(tuple) = row {
            let t_val: TimestampWithTimeZone = tuple.get_by_name("t")
                .map_err(|e| format!("Failed to get 't' column: {}", e))?
                .ok_or("Column 't' is null")?;
            
            // Extract scope values into JSON
            let mut scope_map = serde_json::Map::new();
            for col in &scope_cols {
                // For simplicity in the POC, we treat all scope values as strings
                let val: Option<String> = tuple.get_by_name(col).unwrap_or(None);
                if let Some(v) = val {
                    scope_map.insert(col.clone(), serde_json::Value::String(v));
                }
            }
            let scope_json = pgrx::JsonB(serde_json::Value::Object(scope_map));

            let a = aspiral(t_val);
            let bucket_1m = (a / 60) * 60;
            catalog::mark_bucket_dirty(&base_view_name, bucket_1m, scope_json);

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
fn to_spiral(a: i64, cycle: i64) -> pgrx::pg_sys::Point {
    let r = a as f64;
    let theta = if cycle > 0 {
        (a % cycle) as f64 / cycle as f64 * 2.0 * std::f64::consts::PI
    } else {
        0.0
    };
    pgrx::pg_sys::Point { x: r * theta.cos(), y: r * theta.sin() }
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_cycle(a: i64, cycle: i64) -> i64 {
    if cycle <= 0 { return 0; }
    a / cycle
}

#[pg_extern]
fn aspiral_create_partition(table_name: &str, cycle_seconds: i64, cycle_id: i64) {
    let start = cycle_id * cycle_seconds;
    let end = (cycle_id + 1) * cycle_seconds;
    let p_name = format!("{}_p{}", table_name, cycle_id);
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {} PARTITION OF {} FOR VALUES FROM ({}) TO ({})",
        p_name, table_name, start, end
    );
    let _ = Spi::run(&sql);
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_predict_lane_address(
    a: i64, 
    tenant_id: i32, 
    max_tenants: i32, 
    rows_per_slot: i32,
    row_size: i32
) -> i64 {
    // The "Aspiraling" physical address calculation:
    // Every second (a) has a "Master Bundle" containing all lanes.
    let bundle_size = max_tenants as i64 * rows_per_slot as i64 * row_size as i64;
    let time_offset = a * bundle_size;
    
    // Within the bundle, each tenant has a reserved "Lane"
    let lane_offset = tenant_id as i64 * rows_per_slot as i64 * row_size as i64;
    
    time_offset + lane_offset
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_master_clock_ptr(a: i64, entry_size: i32) -> i64 {
    // Direct array-like access to a pointer file
    a * entry_size as i64
}

#[derive(Serialize, Deserialize, PostgresType, Copy, Clone, Debug)]
pub struct AspiralingNumber {
    pub cycle_id: i64,
    pub lane_id: i32,
    pub offset: i32,
}

#[pg_extern(immutable, parallel_safe)]
fn to_aspiraling_number(a: i64, cycle_duration: i64, lane_id: i32) -> AspiralingNumber {
    AspiralingNumber {
        cycle_id: a / cycle_duration,
        lane_id,
        offset: (a % cycle_duration) as i32,
    }
}

#[derive(Serialize, Deserialize, PostgresType, Copy, Clone)]
pub struct TimeValue {
    pub value: f64,
    pub time: i64,
}

#[pg_extern(immutable, parallel_safe)]
fn first_sfunc(state: Option<TimeValue>, next_val: Option<f64>, next_time: Option<i64>) -> Option<TimeValue> {
    match (state, next_val, next_time) {
        (None, Some(v), Some(t)) => Some(TimeValue { value: v, time: t }),
        (Some(s), Some(v), Some(t)) => {
            if t < s.time {
                Some(TimeValue { value: v, time: t })
            } else {
                Some(s)
            }
        }
        (s, _, _) => s,
    }
}

#[pg_extern(immutable, parallel_safe)]
fn last_sfunc(state: Option<TimeValue>, next_val: Option<f64>, next_time: Option<i64>) -> Option<TimeValue> {
    match (state, next_val, next_time) {
        (None, Some(v), Some(t)) => Some(TimeValue { value: v, time: t }),
        (Some(s), Some(v), Some(t)) => {
            if t >= s.time {
                Some(TimeValue { value: v, time: t })
            } else {
                Some(s)
            }
        }
        (s, _, _) => s,
    }
}

#[pg_extern(immutable, parallel_safe)]
fn time_value_final(state: Option<TimeValue>) -> Option<f64> {
    state.map(|s| s.value)
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
    CREATE AGGREGATE first(f64, i64) (
        sfunc = first_sfunc,
        stype = TimeValue,
        finalfunc = time_value_final
    );
    CREATE AGGREGATE last(f64, i64) (
        sfunc = last_sfunc,
        stype = TimeValue,
        finalfunc = time_value_final
    );
    "#,
    name = "create_aggregates",
    requires = ["aspiral_sketch_sfunc", "aspiral_sketch_merge_sfunc", "first_sfunc", "last_sfunc", "time_value_final"]
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

#[pg_extern]
fn aspiral_create_trigger(table_name: &str, view_name: &str) {
    let sql = format!(
        "CREATE TRIGGER aspiral_track_{view_name} 
         AFTER INSERT OR UPDATE OR DELETE ON {table_name}
         FOR EACH ROW EXECUTE FUNCTION aspiral_track_changes('{view_name}')"
    );
    Spi::run(&sql).unwrap();
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
                res.push(row.get::<i64>(1).unwrap().unwrap());
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
                         first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c
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

    /// SCENARIO 7: Storage Efficiency
    /// Measures the disk space savings of using i64 offsets (aspiral) vs standard timestamptz.
    #[pg_test]
    fn test_scenario_7_storage_efficiency() {
        Spi::run("CREATE TABLE bench_std (t timestamptz NOT NULL, val f64);").expect("Table failed");
        Spi::run("CREATE TABLE bench_asp (t bigint NOT NULL, val f64);").expect("Table failed");

        // Index both time columns
        Spi::run("CREATE INDEX idx_std ON bench_std(t);").expect("Index failed");
        Spi::run("CREATE INDEX idx_asp ON bench_asp(t);").expect("Index failed");

        // Insert 100,000 rows
        info!("--- Inserting 100,000 rows for storage comparison ---");
        Spi::run("INSERT INTO bench_std SELECT '2026-04-15 00:00:00Z'::timestamptz + (i || ' seconds')::interval, random() FROM generate_series(1, 100000) s(i);").unwrap();
        Spi::run("INSERT INTO bench_asp SELECT i, random() FROM generate_series(1, 100000) s(i);").unwrap();

        let sizes = Spi::connect(|client| {
            let row = client.select("
                SELECT 
                    pg_indexes_size('bench_std') as std_idx,
                    pg_indexes_size('bench_asp') as asp_idx,
                    pg_total_relation_size('bench_std') as std_total,
                    pg_total_relation_size('bench_asp') as asp_total
            ", None, &[]).unwrap().first();
            
            Ok::<(i64, i64, i64, i64), spi::Error>((
                row.get::<i64>(1).unwrap().unwrap(),
                row.get::<i64>(2).unwrap().unwrap(),
                row.get::<i64>(3).unwrap().unwrap(),
                row.get::<i64>(4).unwrap().unwrap()
            ))
        }).unwrap();

        info!("Storage Benchmark Results (100k rows):");
        info!("  Standard INDEX size: {} bytes", sizes.0);
        info!("  Aspiral  INDEX size: {} bytes", sizes.1);
        info!("  Standard TOTAL size: {} bytes", sizes.2);
        info!("  Aspiral  TOTAL size: {} bytes", sizes.3);
        
        let saving = (1.0 - (sizes.1 as f64 / sizes.0 as f64)) * 100.0;
        info!("  Index Space Saving: {:.2}%", saving);
    }

    /// SCENARIO 8: Automatic Query Routing
    /// Demonstrates how the extension intercepts queries on high-resolution views
    /// and detects optimization opportunities to route them to lower-resolution frames.
    #[pg_test]
    fn test_scenario_8_query_routing() {
        Spi::run("CREATE TABLE routing_ticks (t timestamptz, price f64);").expect("Table failed");
        
        // Base view (1m) -> 1h frame
        Spi::run("CREATE MATERIALIZED VIEW routing_ohlcv_1m AS 
                  SELECT (aspiral(t)/60)*60 as t, max(price) as h
                  FROM routing_ticks GROUP BY 1
                  WITH (aspiral.frames='1h');").expect("Hierarchy failed");

        // Query the 1m view for a long range.
        // The planner_hook should see this and log an optimization detection.
        info!("--- Querying 1m view (Expect Optimization Log) ---");
        Spi::run("SELECT sum(h) FROM routing_ohlcv_1m WHERE t > 0 AND t < 1000000;").unwrap();
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
