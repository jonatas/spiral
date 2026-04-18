use pgrx::prelude::*;
use pgrx::guc::{GucRegistry, GucSetting, GucContext, GucFlags};
use serde::{Deserialize, Serialize};
use std::ffi::CStr;
use pgrx::AllocatedByPostgres;

mod hooks;
mod catalog;
mod rollup;
mod tam;
mod storage;

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

#[pg_extern(immutable, parallel_safe)]
fn aspiral_predict_lane_address(
    a: i64, 
    tenant_id: i32, 
    max_tenants: i32, 
    rows_per_slot: i32,
    row_size: i32
) -> i64 {
    let bundle_size = max_tenants as i64 * rows_per_slot as i64 * row_size as i64;
    let time_offset = a * bundle_size;
    let lane_offset = tenant_id as i64 * rows_per_slot as i64 * row_size as i64;
    time_offset + lane_offset
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
            if t < s.time { Some(TimeValue { value: v, time: t }) } else { Some(s) }
        }
        (s, _, _) => s,
    }
}

#[pg_extern(immutable, parallel_safe)]
fn last_sfunc(state: Option<TimeValue>, next_val: Option<f64>, next_time: Option<i64>) -> Option<TimeValue> {
    match (state, next_val, next_time) {
        (None, Some(v), Some(t)) => Some(TimeValue { value: v, time: t }),
        (Some(s), Some(v), Some(t)) => {
            if t >= s.time { Some(TimeValue { value: v, time: t }) } else { Some(s) }
        }
        (s, _, _) => s,
    }
}

#[pg_extern(immutable, parallel_safe)]
fn time_value_final(state: Option<TimeValue>) -> Option<f64> {
    state.map(|s| s.value)
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_sketch_sfunc(state: Option<Vec<u8>>, next: Option<f64>) -> Option<Vec<u8>> {
    let digest = match state {
        Some(bytes) => bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100)),
        None => tdigest::TDigest::new_with_size(100),
    };
    if let Some(val) = next {
        Some(bincode::serialize(&digest.merge_unsorted(vec![val])).unwrap())
    } else {
        Some(bincode::serialize(&digest).unwrap())
    }
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_sketch_merge_sfunc(state: Option<Vec<u8>>, next: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let digest = match state {
        Some(bytes) => bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100)),
        None => tdigest::TDigest::new_with_size(100),
    };
    if let Some(bytes) = next {
        let other: tdigest::TDigest = bincode::deserialize(&bytes).unwrap_or_else(|_| tdigest::TDigest::new_with_size(100));
        Some(bincode::serialize(&tdigest::TDigest::merge_digests(vec![digest, other])).unwrap())
    } else {
        Some(bincode::serialize(&digest).unwrap())
    }
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_quantile(sketch: Option<Vec<u8>>, q: f64) -> Option<f64> {
    sketch.and_then(|bytes| {
        let digest: tdigest::TDigest = bincode::deserialize(&bytes).ok()?;
        Some(digest.estimate_quantile(q))
    })
}

#[pg_trigger]
fn aspiral_track_changes<'a>(trigger: &'a pgrx::PgTrigger<'a>) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, String> {
    let base_view_name = trigger.extra_args()
        .map_err(|e| format!("Failed to get trigger args: {}", e))?
        .first()
        .cloned()
        .ok_or("Missing base_view_name in trigger arguments")?;
    
    let (scope_cols, _) = catalog::get_metadata(&base_view_name).unwrap_or((vec![], 0));

    let process_row = |row: Option<PgHeapTuple<'a, AllocatedByPostgres>>| -> Result<(), String> {
        if let Some(tuple) = row {
            let t_val: TimestampWithTimeZone = tuple.get_by_name("t")
                .map_err(|e| format!("Failed to get 't' column: {}", e))?
                .ok_or("Column 't' is null")?;
            
            let mut scope_map = serde_json::Map::new();
            for col in &scope_cols {
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

// Consolidate all custom SQL into one file, but ensure dependencies are defined by pgrx first
extension_sql_file!(
    "../sql/aspiral.sql", 
    name = "create_aspiral_entities",
    requires = [
        aspiral_sketch_sfunc,
        aspiral_sketch_merge_sfunc,
        first_sfunc,
        last_sfunc,
        time_value_final,
        AspiralingNumber,
        TimeValue
    ]
);

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    #[pg_test]
    fn test_scenario_9_massive_benchmark() {
        Spi::run("CREATE TABLE benchmark_delta (t bigint, tenant_id bigint, price double precision);").unwrap();
        Spi::run("INSERT INTO benchmark_delta SELECT (i / 100), (i % 100), random() * 100 FROM generate_series(0, 999_999) s(i);").unwrap();
        Spi::run("SELECT public.aspiral_pack_delta('benchmark_delta', 999_999);").unwrap();
        let val = Spi::get_one::<f64>("SELECT public.aspiral_read_main(999_999, 5, 50)").unwrap().unwrap();
        assert!(val >= 0.0 && val <= 100.0);
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
