use pgrx::prelude::*;
use std::cell::Cell;

pub mod catalog;
pub mod hooks;
pub mod rollup;
pub mod bgworker;
pub mod stats;
pub mod tam;
pub mod storage;

pgrx::pg_module_magic!();

extension_sql_file!("../sql/aspiral.sql", name = "aspiral_setup");

thread_local! {
    pub static SKIP_ACCELERATION: Cell<bool> = const { Cell::new(false) };
}

#[pg_guard]
pub unsafe extern "C-unwind" fn _PG_init() {
    hooks::init_hooks();
}

#[pg_extern]
fn aspiral_zorder(t: i64, dimensions: Vec<Option<String>>) -> i64 {
    let mut x = t as u32;
    let mut y = 0u32;
    for (i, dim) in dimensions.iter().enumerate() {
        if let Some(d) = dim {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::Hasher;
            hasher.write(d.as_bytes());
            let hash = hasher.finish() as u32;
            y ^= hash << (i % 8);
        }
    }
    
    x = (x | (x << 16)) & 0x0000FFFF;
    x = (x | (x << 8)) & 0x00FF00FF;
    x = (x | (x << 4)) & 0x0F0F0F0F;
    x = (x | (x << 2)) & 0x33333333;
    x = (x | (x << 1)) & 0x55555555;

    y = (y | (y << 16)) & 0x0000FFFF;
    y = (y | (y << 8)) & 0x00FF00FF;
    y = (y | (y << 4)) & 0x0F0F0F0F;
    y = (y | (y << 2)) & 0x33333333;
    y = (y | (y << 1)) & 0x55555555;

    (x as i64) | ((y as i64) << 1)
}

#[pg_extern]
fn aspiral_is_loaded() -> bool {
    true
}

#[pg_extern]
fn aspiral(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    let micros = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    // PostgreSQL epoch is 2000-01-01. Unix epoch is 1970-01-01.
    // Offset is 946684800 seconds.
    (micros / 1000000) + 946684800
}

#[pg_extern]
fn cluster_table(table_name: &str, time_col: &str, dimensions: Vec<String>) {
    cluster_table_internal(table_name, time_col, dimensions);
}

pub fn cluster_table_internal(table_name: &str, time_col: &str, dimensions: Vec<String>) {
    let dims = dimensions.iter().map(|d| format!("\"{}\"", d)).collect::<Vec<_>>().join(", ");
    let index_name = format!("idx_z_{}", table_name);
    let sql = format!(
        "CREATE INDEX IF NOT EXISTS \"{index_name}\" ON \"{table_name}\" (aspiral_zorder(aspiral(\"{time_col}\"), ARRAY[{dims}]::text[]));
         CLUSTER \"{table_name}\" USING \"{index_name}\";",
        index_name = index_name, table_name = table_name, time_col = time_col, dims = dims
    );
    let _ = Spi::run(&sql);
}

#[pg_extern]
fn refresh_incremental(view_name: &str, extra_where: default!(Option<String>, "NULL")) -> bool {
    let metadata_all = Spi::get_one::<i64>("SELECT count(*) FROM aspiral.metadata").unwrap().unwrap_or(0);
    notice!("Aspiral: refresh_incremental starting for '{}', total metadata rows: {}", view_name, metadata_all);
    
    let metadata = catalog::get_metadata(view_name);
    if metadata.is_none() { 
        notice!("Aspiral: refresh_incremental failed - no metadata for '{}' (total metadata rows: {})", view_name, metadata_all);
        return false; 
    }
    let metadata = metadata.unwrap();
    let frame_seconds = metadata.frame_seconds;
    let parent_view = metadata.parent_view;
    let scope_cols_raw = metadata.scope_columns;
    let scope_cols: Vec<String> = scope_cols_raw.iter().map(|c| format!("\"{}\"", c)).collect();
    
    notice!("Aspiral: refresh_incremental starting for '{}', parent_view='{}'", view_name, parent_view);
    
    let changelog_key = metadata.base_view.clone();

    let source_table = if parent_view == "BASE" {
        let res = Spi::get_one_with_args::<String>(
            "SELECT base_view FROM aspiral.metadata WHERE view_name = $1",
            &[unsafe { pgrx::datum::DatumWithOid::new(view_name.into_datum().unwrap(), pg_sys::TEXTOID) }]
        );
        
        match res {
            Ok(Some(v)) => {
                notice!("Aspiral: refresh_incremental {} source_table lookup result: {}", view_name, v);
                v
            },
            Ok(None) => {
                notice!("Aspiral: refresh_incremental {} source_table lookup result: NONE", view_name);
                panic!("No base_view found for {}", view_name);
            },
            Err(e) => {
                notice!("Aspiral: refresh_incremental {} source_table get_one error: {:?}", view_name, e);
                panic!("Error getting base_view for {}", view_name);
            }
        }
    } else {
        parent_view.clone()
    };

    notice!("Aspiral: refresh_incremental {} from {}", view_name, source_table);

    let cols_query = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped", view_name.replace("\"", "\"\""));
    let all_cols: Vec<String> = Spi::connect(|client| {
        Ok::<Vec<String>, spi::Error>(client.select(&cols_query, None, &[])?.map(|r| r.get::<String>(1).unwrap().unwrap()).collect())
    }).unwrap_or_default();

    if all_cols.is_empty() {
        notice!("Aspiral: refresh_incremental failed - no columns for '{}'", view_name);
        return false;
    }

    let update_cols: Vec<String> = all_cols.iter()
        .filter(|&c| c != "t" && !scope_cols_raw.contains(c))
        .map(|c| format!("\"{}\"", c)).collect();

    let (sql_child, _) = rollup::derive_child_sql(view_name, &source_table, frame_seconds, &scope_cols_raw);
    let select_part = sql_child.split("SELECT").nth(1).unwrap().split("FROM").next().unwrap().trim();

    let mut on_clause = vec!["target.t::timestamptz = source.t::timestamptz".to_string()];
    for col in &scope_cols { on_clause.push(format!("target.{} = source.{}", col, col)); }
    let update_set = update_cols.iter().map(|c| format!("{} = source.{}", c, c)).collect::<Vec<_>>().join(", ");

    let mut source_where = if scope_cols_raw.is_empty() {
        notice!("Aspiral: refresh_incremental joining changelog for '{}' (no scope)", changelog_key);
        format!(
            "JOIN aspiral.changelog c ON c.base_view = '{changelog_key}' 
                AND aspiral(t) >= (c.t_start/{0})*{0} 
                AND aspiral(t) < ((c.t_end/{0})+1)*{0}
                AND c.scope_values = '{{}}'::jsonb",
            frame_seconds,
            changelog_key = changelog_key.replace("'", "''")
        )
    } else {
        notice!("Aspiral: refresh_incremental joining changelog for '{}' with scope", changelog_key);
        let scope_cols_json = scope_cols_raw.iter().map(|s| format!("'{}', \"{}\"::text", s, s)).collect::<Vec<_>>().join(", ");
        format!(
            "JOIN aspiral.changelog c ON c.base_view = '{changelog_key}' 
                AND aspiral(t) >= (c.t_start/{0})*{0} 
                AND aspiral(t) < ((c.t_end/{0})+1)*{0}
                AND (c.scope_values = '{{}}'::jsonb OR c.scope_values = jsonb_build_object({1}))",
            frame_seconds,
            scope_cols_json,
            changelog_key = changelog_key.replace("'", "''")
        )
    };
    if let Some(ref extra) = extra_where { source_where.push_str(&format!(" WHERE ({})", extra)); }

    let group_by_clause = if scope_cols.is_empty() { "1".to_string() } else { format!("1, {}", scope_cols.join(", ")) };

    let merge_sql = format!(
        "MERGE INTO \"{view_name}\" AS target
            USING (
                SELECT {select_part} FROM \"{source_table}\" 
                {source_where}
                GROUP BY {group_by_clause}
            ) AS source
            ON ({on_clause})
            WHEN MATCHED THEN UPDATE SET {update_set}
            WHEN NOT MATCHED THEN INSERT ({all_cols_joined}) VALUES ({source_cols_joined})",
        view_name = view_name, select_part = select_part, source_table = source_table, source_where = source_where, group_by_clause = group_by_clause, on_clause = on_clause.join(" AND "),
        update_set = if update_set.is_empty() { "t = source.t" } else { &update_set },
        all_cols_joined = all_cols.iter().map(|c| format!("\"{}\"", c)).collect::<Vec<_>>().join(", "),
        source_cols_joined = all_cols.iter().map(|c| format!("source.\"{}\"", c)).collect::<Vec<_>>().join(", ")
    );

    let _changelog_count = Spi::get_one_with_args::<i64>("SELECT count(*) FROM aspiral.changelog WHERE base_view = $1", 
        &[unsafe { pgrx::datum::DatumWithOid::new(changelog_key.clone().into_datum().unwrap(), pg_sys::TEXTOID) }]).unwrap().unwrap_or(0);
    notice!("Aspiral: refresh_incremental {} merge_sql: {}", view_name, merge_sql);
    SKIP_ACCELERATION.with(|s| s.set(true));
    Spi::run(&merge_sql).unwrap();
    SKIP_ACCELERATION.with(|s| s.set(false));

    let result = true;

    if result {
        let children = catalog::get_children(view_name);
        for child in children {
            let _ = refresh_incremental(&child, extra_where.clone());
        }
    }
    result
}

#[pg_extern]
fn aspiral_to_epoch(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    aspiral(t)
}

#[pg_extern]
fn aspiral_from_epoch(epoch: i64) -> pgrx::datum::TimestampWithTimeZone {
    // Unix epoch starts 946684800 seconds before PG epoch
    let micros = (epoch - 946684800) * 1000000;
    unsafe { pgrx::datum::TimestampWithTimeZone::from_datum(micros.into_datum().unwrap(), false).unwrap() }
}

#[pg_extern]
fn aspiral_purge(base_table: &str) {
    let _ = Spi::run(&format!("DELETE FROM aspiral.changelog WHERE base_view = '{}'", base_table.replace("'", "''")));
    notice!("Aspiral: Changelog purged for '{}'", base_table);
}

#[pg_extern]
fn aspiral_status(base_table: &str) -> pgrx::JsonB {
    Spi::connect(|client| {
        let mut status = serde_json::Map::new();
        
        // 1. Hierarchy info
        let mut views = Vec::new();
        let table = client.select("SELECT view_name, frame_seconds, parent_view FROM aspiral.metadata WHERE base_view = $1 ORDER BY frame_seconds ASC", None,
            unsafe { &[pgrx::datum::DatumWithOid::new(base_table.into_datum().unwrap(), pg_sys::TEXTOID)] })?;
        
        for row in table {
            let mut v_info = serde_json::Map::new();
            let v_name = row.get::<String>(1)?.unwrap();
            v_info.insert("frame_seconds".to_string(), serde_json::Value::from(row.get::<i32>(2)?.unwrap()));
            v_info.insert("parent".to_string(), serde_json::Value::from(row.get::<String>(3)?.unwrap()));
            
            // Get row count for each view
            let count = client.select(&format!("SELECT count(*) FROM \"{}\"", v_name.replace("\"", "\"\"")), Some(1), &[])?.get_one::<i64>()?.unwrap_or(0);
            v_info.insert("row_count".to_string(), serde_json::Value::from(count));
            
            views.push(serde_json::Value::Object(v_info));
        }
        status.insert("hierarchy".to_string(), serde_json::Value::Array(views));

        // 2. Dirtiness info
        let dirty_count = client.select("SELECT count(*) FROM aspiral.changelog WHERE base_view = $1", Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(base_table.into_datum().unwrap(), pg_sys::TEXTOID)] })?.get_one::<i64>()?.unwrap_or(0);
        status.insert("dirty_segments_count".to_string(), serde_json::Value::from(dirty_count));

        Ok::<pgrx::JsonB, spi::Error>(pgrx::JsonB(serde_json::Value::Object(status)))
    }).unwrap()
}

#[pg_extern(name = "aspiral_refresh")]
fn aspiral_refresh(view_name: &str, where_clause: default!(Option<&str>, "NULL")) {
    hooks::reactive_refresh(view_name, where_clause.map(|s| s.to_string()));
}

#[pg_extern]
fn aspiral_register_view(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str, scope_columns: Vec<String>) {
    notice!("Aspiral: registering view '{}', parent='{}', base='{}'", view_name, parent_view, base_view);
    let source_table = if parent_view == "BASE" { base_view } else { parent_view };
    let (sql, sources) = rollup::derive_child_sql(view_name, source_table, frame_seconds, &scope_columns);
    
    if !sql.is_empty() {
        let create_sql = format!("CREATE TABLE IF NOT EXISTS {} AS SELECT * FROM ({}) s LIMIT 0", view_name, sql.split(" AS ").nth(1).unwrap().split(';').next().unwrap());
        let _ = Spi::run(&create_sql);
    }

    catalog::insert_metadata(view_name, parent_view, frame_seconds, base_view, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
    notice!("Aspiral: view '{}' metadata inserted", view_name);
    for src in sources { catalog::insert_source(view_name, base_view, frame_seconds, &src.base_column, &src.formula, &src.mat_column, pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))); }
    if parent_view == "BASE" {
        for event in &["INSERT", "UPDATE", "DELETE"] {
            let mut transition = String::new();
            if *event == "UPDATE" {
                transition.push_str("REFERENCING NEW TABLE AS new_table OLD TABLE AS old_table ");
            } else if *event == "INSERT" {
                transition.push_str("REFERENCING NEW TABLE AS new_table ");
            } else if *event == "DELETE" {
                transition.push_str("REFERENCING OLD TABLE AS old_table ");
            }

            let trigger_sql = format!(
                "CREATE TRIGGER aspiral_track_{base_view}_{event_lower} 
                 AFTER {event} ON \"{base_view}\" 
                 {transition}
                 FOR EACH STATEMENT EXECUTE FUNCTION aspiral.track_changes_stmt('{base_view}')",
                base_view = base_view, event = event, event_lower = event.to_lowercase(),
                transition = transition
            );
            let _ = Spi::run(&trigger_sql);
        }
    }
    notice!("Aspiral: view '{}' registration complete", view_name);
}

pub fn get_kickoff_epoch() -> i64 {
    Spi::get_one::<i64>("SELECT aspiral(COALESCE(current_setting('aspiral.kickoff_date', true), '1970-01-01')::timestamptz)").unwrap_or(Some(0)).unwrap()
}

pub fn get_minimal_pace() -> f64 {
    Spi::get_one::<f64>("SELECT COALESCE(current_setting('aspiral.minimal_pace', true), '60')::numeric::float8").unwrap_or(Some(60.0)).unwrap()
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;
    use crate::rollup;

    #[pg_test]
    fn test_regular_tables_ignored() {
        Spi::run("CREATE TABLE regular_table (t timestamptz, val double precision);").unwrap();
        let count = Spi::get_one::<i64>("SELECT count(*) FROM aspiral.metadata WHERE base_view = 'regular_table'").unwrap().unwrap();
        assert_eq!(count, 0);

        let trigger_exists = Spi::get_one::<bool>("SELECT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid = 'regular_table'::regclass AND tgname LIKE 'aspiral%')").unwrap().unwrap();
        assert!(!trigger_exists);
    }

    #[pg_test]
    fn test_benchmark_acceleration_1m() {
        Spi::run("SET aspiral.kickoff_date = '2026-04-15';").unwrap();
        Spi::run("CREATE TABLE stress_raw (t timestamptz NOT NULL, val double precision);").unwrap();

        Spi::run("SELECT aspiral_register_view('stress_raw_ohlcv_1m', 'BASE', 60, 'stress_raw', ARRAY[]::text[]);").unwrap();
        Spi::run("SELECT aspiral_register_view('stress_raw_ohlcv_1h', 'stress_raw_ohlcv_1m', 3600, 'stress_raw', ARRAY[]::text[]);").unwrap();
        Spi::run("SELECT aspiral_register_view('stress_raw_ohlcv_1d', 'stress_raw_ohlcv_1h', 86400, 'stress_raw', ARRAY[]::text[]);").unwrap();

        // Ingest 100k rows
        Spi::run("INSERT INTO stress_raw (t, val) SELECT '2026-04-15 00:00:00Z'::timestamptz + (n || ' seconds')::interval, random() * 100 FROM generate_series(0, 100000) n;").unwrap();

        let count_raw = Spi::get_one::<i64>("SELECT count(*) FROM stress_raw WHERE aspiral(t) >= 1776211200 AND aspiral(t) < 1776311220").unwrap().unwrap_or(0);
        notice!("Aspiral TEST: raw count matching 1776211200..1776311220 is {}", count_raw);

        let count_raw_all = Spi::get_one::<i64>("SELECT count(*) FROM stress_raw").unwrap().unwrap_or(0);
        let min_t = Spi::get_one::<i64>("SELECT MIN(aspiral(t)) FROM stress_raw").unwrap().unwrap_or(0);
        let max_t = Spi::get_one::<i64>("SELECT MAX(aspiral(t)) FROM stress_raw").unwrap().unwrap_or(0);
        notice!("Aspiral TEST: total raw count is {}, min_t={}, max_t={}", count_raw_all, min_t, max_t);

        Spi::run("SELECT aspiral_refresh('stress_raw_ohlcv_1m');").unwrap();
        Spi::run("SELECT aspiral_refresh('stress_raw_ohlcv_1h');").unwrap();
        Spi::run("SELECT aspiral_refresh('stress_raw_ohlcv_1d');").unwrap();

        let start = std::time::Instant::now();
        let total_sum = Spi::get_one::<f64>("SELECT sum(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-20 00:00:00Z'").unwrap().expect("Acceleration failed to return data");
        let duration = start.elapsed();

        notice!("Accelerated Query (100k rows) took: {:?}", duration);
        assert!(total_sum > 0.0);
        assert!(duration.as_millis() < 100); 
    }}

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
    }
    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries = 'aspiral'"]
    }
}
