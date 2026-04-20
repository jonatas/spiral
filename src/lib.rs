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
mod stats;
mod bgworker;

::pgrx::pg_module_magic!(name, version);

static KICKOFF_DATE: GucSetting<Option<std::ffi::CString>> = GucSetting::<Option<std::ffi::CString>>::new(None);

#[no_mangle]
pub unsafe extern "C" fn _PG_init() {
    hooks::init_hooks();
    bgworker::init_worker();

    GucRegistry::define_string_guc(
        CStr::from_bytes_with_nul(b"aspiral.kickoff_date\0").unwrap(),
        CStr::from_bytes_with_nul(b"Point zero for the aspiral timeline (YYYY-MM-DD)\0").unwrap(),
        CStr::from_bytes_with_nul(b"The first day the system starts operating as the point zero.\0").unwrap(),
        &KICKOFF_DATE,
        GucContext::Suset,
        GucFlags::default(),
    );
}

use std::cell::RefCell;

thread_local! {
    static KICKOFF_CACHE: RefCell<Option<(String, i64)>> = RefCell::new(None);
}

fn get_kickoff_epoch() -> i64 {
    let kickoff = KICKOFF_DATE.get();
    let kickoff_str = match kickoff {
        Some(s) => s.to_string_lossy().into_owned(),
        None => "2026-04-15".to_string(),
    };

    if let Some((cached_str, cached_val)) = KICKOFF_CACHE.with(|c| c.borrow().clone()) {
        if cached_str == kickoff_str {
            return cached_val;
        }
    }

    let date_part = if kickoff_str.contains(' ') {
        kickoff_str.split(' ').next().unwrap().to_string()
    } else {
        kickoff_str.clone()
    };

    // Use a robust way to get epoch from YYYY-MM-DD
    let sql = format!("SELECT extract(epoch from '{} 00:00:00Z'::timestamptz)::bigint", date_part.replace("'", "''"));
    let val = Spi::get_one::<i64>(&sql).unwrap_or(Some(1776211200)).unwrap_or(1776211200);
    
    KICKOFF_CACHE.with(|c| *c.borrow_mut() = Some((kickoff_str, val)));
    val
}

#[pg_extern(immutable, parallel_safe, name = "aspiral")]
fn aspiral(t: TimestampWithTimeZone) -> i64 {
    let micros_since_pg_epoch = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    let pg_epoch_unix: i64 = 946684800;
    let unix_seconds = pg_epoch_unix + (micros_since_pg_epoch / 1_000_000);
    unix_seconds - get_kickoff_epoch()
}

#[pg_extern(immutable, parallel_safe, name = "aspiral")]
fn aspiral_i64(t: i64) -> i64 {
    t
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
    Spi::get_one::<i64>("SELECT aspiral(now())").unwrap_or(Some(0)).unwrap_or(0)
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
    
    let metadata = catalog::get_metadata(&base_view_name);
    let scope_cols = metadata.map(|m| m.scope_columns).unwrap_or_default();

    let process_row = |row: Option<PgHeapTuple<'a, AllocatedByPostgres>>| -> Result<(), String> {
        if let Some(tuple) = row {
            let t_val: TimestampWithTimeZone = tuple.get_by_name("t")
                .map_err(|e| format!("Failed to get 't' column: {}", e))?
                .ok_or("Column 't' is null")?;
            
            let mut scope_map = serde_json::Map::new();
            for col in &scope_cols {
                // Try to get as string first, then as i32, then as i64
                let val: Option<String> = if let Ok(v) = tuple.get_by_name::<String>(col) {
                    v
                } else if let Ok(v) = tuple.get_by_name::<i32>(col) {
                    v.map(|i| i.to_string())
                } else if let Ok(v) = tuple.get_by_name::<i64>(col) {
                    v.map(|i| i.to_string())
                } else {
                    None
                };

                if let Some(v) = val {
                    scope_map.insert(col.to_string(), serde_json::Value::String(v));
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

fn split_by_2(a: u32) -> u64 {
    let mut x = a as u64;
    x = (x | x << 16) & 0x0000ffff0000ffff;
    x = (x | x << 8)  & 0x00ff00ff00ff00ff;
    x = (x | x << 4)  & 0x0f0f0f0f0f0f0f0f;
    x = (x | x << 2)  & 0x3333333333333333;
    x = (x | x << 1)  & 0x5555555555555555;
    x
}

fn split_by_3(a: u32) -> u64 {
    let mut x = (a & 0x1fffff) as u64; // 21 bits
    x = (x | x << 32) & 0x1f00000000ffff;
    x = (x | x << 16) & 0x1f0000ff0000ff;
    x = (x | x << 8)  & 0x100f00f00f00f00f;
    x = (x | x << 4)  & 0x10c30c30c30c30c3;
    x = (x | x << 2)  & 0x1249249249249249;
    x
}

fn split_by_4(a: u32) -> u64 {
    let mut x = (a & 0xffff) as u64; // 16 bits
    x = (x | x << 24) & 0x0000ff00000000ff;
    x = (x | x << 12) & 0x000f000f000f000f;
    x = (x | x << 6)  & 0x0303030303030303;
    x = (x | x << 3)  & 0x1111111111111111;
    x
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_zorder(t: i64, ids: Vec<String>) -> i64 {
    let t_scaled = (t / 3600) as u32;
    
    // Hash text IDs into u32 for bit interleaving
    let hashed_ids: Vec<u32> = ids.iter().map(|s| {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish() as u32
    }).collect();

    let res: u64 = match hashed_ids.len() {
        1 => split_by_2(t_scaled) | (split_by_2(hashed_ids[0]) << 1),
        2 => split_by_3(t_scaled) | (split_by_3(hashed_ids[0]) << 1) | (split_by_3(hashed_ids[1]) << 2),
        3 => split_by_4(t_scaled) | (split_by_4(hashed_ids[0]) << 1) | (split_by_4(hashed_ids[1]) << 2) | (split_by_4(hashed_ids[2]) << 3),
        _ => {
            if hashed_ids.is_empty() {
                t as u64
            } else {
                split_by_2(t_scaled) | (split_by_2(hashed_ids[0]) << 1)
            }
        }
    };
    res as i64
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_zorder_fine(t: i64, scale_seconds: i32, ids: Vec<String>) -> i64 {
    let t_scaled = if scale_seconds > 0 { (t / scale_seconds as i64) as u32 } else { t as u32 };
    
    let hashed_ids: Vec<u32> = ids.iter().map(|s| {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        s.hash(&mut hasher);
        hasher.finish() as u32
    }).collect();

    let res: u64 = match hashed_ids.len() {
        1 => split_by_2(t_scaled) | (split_by_2(hashed_ids[0]) << 1),
        2 => split_by_3(t_scaled) | (split_by_3(hashed_ids[0]) << 1) | (split_by_3(hashed_ids[1]) << 2),
        3 => split_by_4(t_scaled) | (split_by_4(hashed_ids[0]) << 1) | (split_by_4(hashed_ids[1]) << 2) | (split_by_4(hashed_ids[2]) << 3),
        _ => {
            if hashed_ids.is_empty() {
                t as u64
            } else {
                split_by_2(t_scaled) | (split_by_2(hashed_ids[0]) << 1)
            }
        }
    };
    res as i64
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_hilbert_2d(x: i32, y: i32) -> i64 {
    let mut x = x as u32;
    let mut y = y as u32;
    let mut d = 0u64;
    let mut s = 1u32 << 30; 
    while s > 0 {
        let rx = if (x & s) > 0 { 1 } else { 0 };
        let ry = if (y & s) > 0 { 1 } else { 0 };
        d += s as u64 * s as u64 * ((3 * rx) ^ ry) as u64;
        
        // rot
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    d as i64
}

#[pg_extern(immutable, parallel_safe)]
fn aspiral_zorder_adaptive(t: i64, table_name: &str, ids: Vec<String>) -> i64 {
    // Determine scale dynamically from table stats
    // Note: We use Spi::get_one instead of get_one_with_args for the table name
    let query = format!(
        "SELECT GREATEST(1, (EXTRACT(EPOCH FROM MAX(t)) - EXTRACT(EPOCH FROM MIN(t)))::int / 1000) FROM (SELECT t FROM {} LIMIT 1000) s",
        table_name.replace("'", "''")
    );
    let scale = Spi::get_one::<i32>(&query).unwrap_or(Some(60)).unwrap_or(60);

    aspiral_zorder_fine(t, scale, ids)
}

#[pg_extern(name = "aspiral_cluster_table")]
fn aspiral_cluster_table(table_name: &str, time_col: &str, dimension_cols: Vec<String>) {
    cluster_table_internal(table_name, time_col, dimension_cols);
}

pub fn cluster_table_internal(table_name: &str, time_col: &str, dimension_cols: Vec<String>) {
    let index_name = format!("idx_aspiral_z_{}", table_name.replace(".", "_"));
    
    // Construct the dimensions array part
    let dimensions_joined = dimension_cols.iter()
        .map(|s| format!("\"{}\"", s))
        .collect::<Vec<String>>()
        .join(", ");

    // Smart detection for time_col type to avoid AT TIME ZONE on bigints
    // Safely check if relation exists before calling ::regclass
    let exists = Spi::get_one_with_args::<bool>("SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = $1::text)", &[Some(table_name.into_datum()).into()]).unwrap_or(Some(false)).unwrap_or(false);
    if !exists { return; }

    let is_bigint = unsafe {
        Spi::get_one_with_args::<bool>(
            "SELECT a.atttypid = 'int8'::regtype OR a.atttypid = 'bigint'::regtype 
             FROM pg_attribute a 
             JOIN pg_class c ON a.attrelid = c.oid
             WHERE c.relname = $1 AND a.attname = $2",
            &[
                pgrx::datum::DatumWithOid::new(table_name.into_datum(), pg_sys::TEXTOID),
                pgrx::datum::DatumWithOid::new(time_col.into_datum(), pg_sys::TEXTOID),
            ]
        ).unwrap_or(Some(false)).unwrap_or(false)
    };

    let time_expr = if is_bigint {
        format!("\"{}\"", time_col)
    } else {
        format!("EXTRACT(EPOCH FROM (\"{time_col}\" AT TIME ZONE 'UTC'))::bigint")
    };

    let query = format!(
        "CREATE INDEX \"{}\" ON \"{}\" (
            aspiral_zorder(
                {time_expr}, 
                ARRAY[{dimensions_joined}]::text[]
            )
        )",
        index_name, table_name
    );

    pgrx::notice!("Creating Z-Order index: {}", index_name);
    
    let result = pgrx::spi::Spi::run(&query);
    match result {
        Ok(_) => pgrx::notice!("Successfully created Z-Order clustering index on {}", table_name),
        Err(e) => pgrx::error!("Failed to create Z-Order index: {}", e),
    }
}

pub fn refresh_incremental(view_name: &str, extra_where: Option<String>) -> bool {
    let result = Spi::connect(|client| {
        // 1. Get metadata for the view
        let meta_row = client.select(
            "SELECT parent_view, frame_seconds, scope_columns, base_view FROM aspiral.metadata WHERE view_name = $1::text",
            Some(1),
            &[Some(view_name.into_datum()).into()]
        )?.first();

        if meta_row.is_empty() { return Ok::<bool, spi::Error>(false); }

        let parent_view: String = meta_row.get::<String>(1)?.unwrap();
        let frame_seconds: i32 = meta_row.get::<i32>(2)?.unwrap();
        let scope_cols_raw: Vec<String> = meta_row.get::<Vec<String>>(3)?.unwrap();
        let root_view: String = meta_row.get::<String>(4)?.unwrap();
        let scope_cols: Vec<String> = scope_cols_raw.iter().map(|s| format!("\"{}\"", s)).collect();

        // 2. Fetch all column names
        let mut all_cols = Vec::new();
        let cols_table = client.select(
            &format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '{}'::regclass AND attnum > 0 AND NOT attisdropped", view_name.replace("'", "''")),
            None,
            &[]
        )?;
        for row in cols_table { all_cols.push(row.get::<String>(1)?.unwrap()); }

        let update_cols: Vec<String> = all_cols.iter()
            .filter(|&c| c != "t" && !scope_cols_raw.contains(c))
            .map(|c| format!("\"{}\"", c)).collect();

        // 3. Construct MERGE
        let sql = rollup::derive_child_sql(view_name, &parent_view, frame_seconds, &scope_cols_raw);
        let select_part = sql.split("SELECT").nth(1).unwrap().split("FROM").next().unwrap().trim();

        let mut on_clause = vec!["target.t = source.t".to_string()];
        for col in &scope_cols { on_clause.push(format!("target.{} = source.{}", col, col)); }
        let update_set = update_cols.iter().map(|c| format!("{} = source.{}", c, c)).collect::<Vec<_>>().join(", ");

        // IMPORTANT: Use root_view for changelog lookup!
        let mut source_where = format!("WHERE (aspiral(t)/{0})*{0} IN (SELECT bucket_t FROM aspiral.changelog WHERE base_view = '{root_view}')", frame_seconds, root_view = root_view.replace("'", "''"));
        if let Some(ref extra) = extra_where { source_where.push_str(&format!(" AND ({})", extra)); }

        let merge_sql = format!(
            "MERGE INTO {view_name} AS target
             USING (
                 SELECT {select_part} FROM {parent_view} 
                 {source_where}
                 GROUP BY 1, {groups}
             ) AS source
             ON ({on_clause})
             WHEN MATCHED THEN UPDATE SET {update_set}
             WHEN NOT MATCHED THEN INSERT ({all_cols_joined}) VALUES ({source_cols_joined})",
            view_name = view_name, select_part = select_part, parent_view = parent_view, source_where = source_where, groups = scope_cols.join(", "), on_clause = on_clause.join(" AND "),
            update_set = if update_set.is_empty() { "t = source.t" } else { &update_set },
            all_cols_joined = all_cols.iter().map(|c| format!("\"{}\"", c)).collect::<Vec<_>>().join(", "),
            source_cols_joined = all_cols.iter().map(|c| format!("source.\"{}\"", c)).collect::<Vec<_>>().join(", ")
        );

        info!("Aspiral: Performing Incremental MERGE for '{}'", view_name);
        let _ = Spi::run(&merge_sql);
        Ok(true)
    }).unwrap_or(false);

    if result {
        let children = catalog::get_children(view_name);
        for child in children {
            refresh_incremental(&child, extra_where.clone());
        }
    }
    result
}

fn where_clause_was_none(opt: Option<String>) -> bool { opt.is_none() }

#[pg_extern(name = "aspiral_refresh")]
fn aspiral_refresh(view_name: &str, where_clause: default!(Option<&str>, "NULL")) {
    hooks::reactive_refresh(view_name, where_clause.map(|s| s.to_string()));
}

#[pg_extern]
fn aspiral_register_view(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str, scope_columns: Vec<String>) {
    catalog::insert_metadata(view_name, parent_view, frame_seconds, base_view, scope_columns.clone());
    // Also create the trigger on the base table if this is a root view
    if parent_view == "BASE" {
        let trigger_sql = format!("CREATE TRIGGER aspiral_track_{} AFTER INSERT OR UPDATE OR DELETE ON \"{}\" FOR EACH ROW EXECUTE FUNCTION aspiral_track_changes('{}')", base_view, base_view, view_name);
        let _ = Spi::run(&trigger_sql);
    }
}

#[pg_extern]
fn aspiral_create_hierarchy(base_name: &str, frames_str: &str, scope_columns: Vec<String>) {
    hooks::generate_hierarchy(base_name, frames_str, scope_columns);
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

    #[pg_test]
    fn test_walkthrough() {
        // Run the walkthrough.sql script (ignoring DROP/CREATE EXTENSION)
        let walkthrough = include_str!("../walkthrough.sql");
        let lines: Vec<&str> = walkthrough.lines()
            .filter(|line| !line.to_uppercase().contains("EXTENSION") && !line.to_uppercase().contains("SCHEMA"))
            .collect();
        let cleaned_sql = lines.join("\n");
        
        // Split by semicolon to run each statement individually
        // Note: This is a simple splitter and might fail on complex SQL with semicolons in strings/blocks.
        // But for walkthrough.sql it should be okay.
        for statement in cleaned_sql.split(';') {
            let stmt = statement.trim();
            if !stmt.is_empty() {
                Spi::run(stmt).expect(&format!("Failed to execute statement: {}", stmt));
            }
        }
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
