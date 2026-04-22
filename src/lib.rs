use pgrx::prelude::*;

pub mod catalog;
pub mod hooks;
pub mod rollup;
pub mod bgworker;
pub mod stats;
pub mod tam;
pub mod storage;

pgrx::pg_module_magic!();

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
fn aspiral(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() }
}

#[pg_trigger]
fn aspiral_track_changes<'a>(
    trigger: &'a pgrx::PgTrigger<'a>,
) -> Result<Option<pgrx::heap_tuple::PgHeapTuple<'a, pgrx::AllocatedByRust>>, spi::Error> {
    let args = trigger.extra_args().map_err(|e| spi::Error::CursorNotFound(e.to_string()))?;
    let root_view = args.first().unwrap();

    let sql = format!(r#"
        DO $body$
        BEGIN
            BEGIN
                INSERT INTO aspiral.changelog (base_view, t_start, t_end, scope_values)
                SELECT '{0}', MIN(aspiral(t)), MAX(aspiral(t)), '{{}}'::jsonb FROM new_table;
            EXCEPTION WHEN OTHERS THEN
            END;
            BEGIN
                INSERT INTO aspiral.changelog (base_view, t_start, t_end, scope_values)
                SELECT '{0}', MIN(aspiral(t)), MAX(aspiral(t)), '{{}}'::jsonb FROM old_table;
            EXCEPTION WHEN OTHERS THEN
            END;
        END $body$;
    "#, root_view.replace("'", "''"));

    let _ = Spi::run(&sql);
    Ok(None)
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
    let metadata = catalog::get_metadata(view_name);
    if metadata.is_none() { return false; }
    let metadata = metadata.unwrap();
    let frame_seconds = metadata.frame_seconds;
    let parent_view = metadata.parent_view;
    let scope_cols_raw = metadata.scope_columns;
    let scope_cols: Vec<String> = scope_cols_raw.iter().map(|c| format!("\"{}\"", c)).collect();
    
    let mut current_meta = catalog::get_metadata(view_name);
    let mut changelog_key = view_name.to_string();
    while let Some(ref m) = current_meta {
        if m.parent_view == "BASE" { break; }
        changelog_key = m.parent_view.clone();
        current_meta = catalog::get_metadata(&changelog_key);
    }

    let result = Spi::connect(|client| {
        let source_table = if parent_view == "BASE" {
            client.select("SELECT base_view FROM aspiral.metadata WHERE view_name = $1", None, 
                unsafe { &[pgrx::datum::DatumWithOid::new(view_name.into_datum().unwrap(), pg_sys::TEXTOID)] })?.get_one::<String>()?.unwrap()
        } else {
            parent_view.clone()
        };

        let cols_query = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped", view_name.replace("\"", "\"\""));
        let all_cols: Vec<String> = client.select(&cols_query, None, &[])?.map(|r| r.get::<String>(1).unwrap().unwrap()).collect();

        if all_cols.is_empty() {
            return Ok::<bool, spi::Error>(false);
        }

        let update_cols: Vec<String> = all_cols.iter()
            .filter(|&c| c != "t" && !scope_cols_raw.contains(c))
            .map(|c| format!("\"{}\"", c)).collect();

        let (sql_child, _) = rollup::derive_child_sql(view_name, &source_table, frame_seconds, &scope_cols_raw);
        let select_part = sql_child.split("SELECT").nth(1).unwrap().split("FROM").next().unwrap().trim();

        let mut on_clause = vec!["target.t = source.t".to_string()];
        for col in &scope_cols { on_clause.push(format!("target.{} = source.{}", col, col)); }
        let update_set = update_cols.iter().map(|c| format!("{} = source.{}", c, c)).collect::<Vec<_>>().join(", ");

        let mut source_where = if scope_cols_raw.is_empty() {
            format!(
                "JOIN aspiral.changelog c ON c.base_view = '{changelog_key}' 
                 AND aspiral(t) >= (c.t_start/{0})*{0} 
                 AND aspiral(t) < ((c.t_end/{0})+1)*{0}
                 AND c.scope_values = '{{}}'::jsonb",
                frame_seconds,
                changelog_key = changelog_key.replace("'", "''")
            )
        } else {
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

        let _ = Spi::run(&merge_sql);
        Ok(true)
    }).unwrap_or(false);

    if result {
        let children = catalog::get_children(view_name);
        for child in children {
            let _ = refresh_incremental(&child, extra_where.clone());
        }
    }
    result
}

#[pg_extern(name = "aspiral_refresh")]
fn aspiral_refresh(view_name: &str, where_clause: default!(Option<&str>, "NULL")) {
    hooks::reactive_refresh(view_name, where_clause.map(|s| s.to_string()));
}

#[pg_extern]
fn aspiral_register_view(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str, scope_columns: Vec<String>) {
    let source_table = if parent_view == "BASE" { base_view } else { parent_view };
    let (sql, sources) = rollup::derive_child_sql(view_name, source_table, frame_seconds, &scope_columns);
    
    if !sql.is_empty() {
        let create_sql = format!("CREATE TABLE IF NOT EXISTS {} AS SELECT * FROM ({}) s LIMIT 0", view_name, sql.split(" AS ").nth(1).unwrap().split(';').next().unwrap());
        let _ = Spi::run(&create_sql);
    }

    catalog::insert_metadata(view_name, parent_view, frame_seconds, base_view, scope_columns.clone(), pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())));
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
                 FOR EACH STATEMENT EXECUTE FUNCTION aspiral_track_changes('{view_name}')",
                base_view = base_view, event = event, event_lower = event.to_lowercase(),
                transition = transition, view_name = view_name
            );
            let _ = Spi::run(&trigger_sql);
        }
    }
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

    #[pg_test]
    fn test_regular_tables_ignored() {
        Spi::run("CREATE TABLE regular_table (t timestamptz, val double precision);").unwrap();
        let count = Spi::get_one::<i64>("SELECT count(*) FROM aspiral.metadata WHERE base_view = 'regular_table'").unwrap().unwrap();
        assert_eq!(count, 0);

        let trigger_exists = Spi::get_one::<bool>("SELECT EXISTS(SELECT 1 FROM pg_trigger WHERE tgrelid = 'regular_table'::regclass AND tgname LIKE 'aspiral%')").unwrap().unwrap();
        assert!(!trigger_exists);
    }

    #[pg_test]
    fn test_hierarchical_cache_acceleration() {
        Spi::run("SET aspiral.kickoff_date = '2026-04-15';").unwrap();
        Spi::run("CREATE TABLE cache_heavy (t timestamptz NOT NULL, val double precision);").unwrap();
        
        Spi::run("SELECT aspiral_register_view('cache_heavy_ohlcv_1m', 'BASE', 60, 'cache_heavy', ARRAY[]::text[]);").unwrap();
        Spi::run("SELECT aspiral_register_view('cache_heavy_ohlcv_1h', 'cache_heavy_ohlcv_1m', 3600, 'cache_heavy', ARRAY[]::text[]);").unwrap();

        Spi::run("INSERT INTO cache_heavy (t, val) VALUES
            ('2026-04-15 10:00:05Z', 10.0),
            ('2026-04-15 10:00:55Z', 20.0),
            ('2026-04-15 10:01:05Z', 30.0);").unwrap();

        Spi::run("INSERT INTO cache_heavy (t, val) VALUES
            ('2026-04-15 11:00:05Z', 40.0),
            ('2026-04-15 11:00:55Z', 50.0);").unwrap();

        Spi::run("SELECT aspiral_refresh('cache_heavy_ohlcv_1m');").unwrap();
        Spi::run("SELECT aspiral_refresh('cache_heavy_ohlcv_1h');").unwrap();

        let total_sum = Spi::get_one::<f64>("SELECT sum(val) FROM cache_heavy WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 12:00:00Z'").unwrap().unwrap();
        assert_eq!(total_sum, 150.0);

        let total_count = Spi::get_one::<i64>("SELECT count(val) FROM cache_heavy WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 12:00:00Z'").unwrap().unwrap();
        assert_eq!(total_count, 5);

        let total_avg = Spi::get_one::<f64>("SELECT avg(val) FROM cache_heavy WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 12:00:00Z'").unwrap().unwrap();
        assert_eq!(total_avg, 30.0);
    }
}

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {
    }
    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries = 'aspiral'"]
    }
}
