use pgrx::prelude::*;
use std::cell::Cell;

pub mod bgworker;
pub mod catalog;
pub mod hooks;
pub mod rollup;
pub mod stats;
pub mod storage;
pub mod tam;

pgrx::pg_module_magic!();

extension_sql_file!("../sql/spiral.sql", name = "spiral_setup");

thread_local! {
    pub static SKIP_ACCELERATION: Cell<bool> = const { Cell::new(false) };
}

#[pg_guard]
pub unsafe extern "C-unwind" fn _PG_init() {
    hooks::init_hooks();
}

#[pg_extern(immutable, parallel_safe)]
fn spiral_zorder(t: i64, dimensions: Vec<Option<String>>) -> i64 {
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

#[pg_extern(immutable, parallel_safe, name = "spiral_zorder")]
fn spiral_zorder_int_array(t: i64, dimensions: Vec<i32>) -> i64 {
    let mut x = t as u32;
    let mut y = 0u32;
    for (i, dim) in dimensions.iter().enumerate() {
        y ^= (*dim as u32) << (i % 8);
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

#[pg_extern(immutable, parallel_safe)]
fn spiral_zorder_3d(x: i64, y: i32, z: i32) -> i64 {
    let mut res = 0i64;
    for i in 0..20 {
        res |= ((x >> i) & 1) << (3 * i);
        res |= (((y as i64) >> i) & 1) << (3 * i + 1);
        res |= (((z as i64) >> i) & 1) << (3 * i + 2);
    }
    res
}

#[pg_extern(immutable, parallel_safe)]
fn spiral_hilbert_2d(x: i32, y: i32) -> i32 {
    let mut res = 0i32;
    for i in 0..15 {
        res |= ((x >> i) & 1) << (2 * i);
        res |= ((y >> i) & 1) << (2 * i + 1);
    }
    res
}

#[pg_extern]
fn spiral_is_loaded() -> bool {
    true
}

#[pg_extern(immutable, parallel_safe)]
fn spiral(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    let micros = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    (micros / 1000000) + 946684800
}

#[pg_extern(immutable, parallel_safe, name = "spiral")]
fn spiral_bigint(t: i64) -> i64 {
    t
}

#[pg_extern]
fn cluster_table(table_name: &str, time_col: &str, dimensions: Vec<String>) {
    cluster_table_internal(table_name, time_col, dimensions);
}

pub fn cluster_table_internal(table_name: &str, time_col: &str, dimensions: Vec<String>) {
    let dims = dimensions
        .iter()
        .map(|d| format!("\"{}\"", d))
        .collect::<Vec<_>>()
        .join(", ");
    let index_name = format!("idx_z_{}", table_name);
    let sql = format!(
        "CREATE INDEX IF NOT EXISTS \"{index_name}\" ON \"{table_name}\" (spiral_zorder(spiral(\"{time_col}\"), ARRAY[{dims}]::text[]));
         CLUSTER \"{table_name}\" USING \"{index_name}\";",
        index_name = index_name, table_name = table_name, time_col = time_col, dims = dims
    );
    let _ = Spi::run(&sql);
}

#[pg_extern]
fn refresh_incremental(
    view_name: &str,
    extra_where: default!(Option<String>, "NULL"),
    depth: default!(i32, 0),
) -> bool {
    if depth > 5 {
        notice!(
            "Spiral: refresh_incremental reached max depth for '{}'",
            view_name
        );
        return false;
    }

    let metadata = catalog::get_metadata(view_name);
    if metadata.is_none() {
        return false;
    }
    let metadata = metadata.unwrap();
    let frame_seconds = metadata.frame_seconds;
    let parent_view = metadata.parent_view;
    let scope_cols_raw = metadata.scope_columns;
    let scope_cols: Vec<String> = scope_cols_raw
        .iter()
        .map(|c| format!("\"{}\"", c))
        .collect();

    let changelog_key = metadata.base_view.clone();

    let source_table = if parent_view == "BASE" {
        let res = Spi::get_one_with_args::<String>(
            "SELECT base_view FROM spiral.metadata WHERE view_name = $1",
            &[unsafe {
                pgrx::datum::DatumWithOid::new(view_name.into_datum().unwrap(), pg_sys::TEXTOID)
            }],
        );

        match res {
            Ok(Some(v)) => v,
            Ok(None) => panic!("No base_view found for {}", view_name),
            Err(_) => panic!("Error getting base_view for {}", view_name),
        }
    } else {
        parent_view.clone()
    };

    let cols_query = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = '\"{}\"'::regclass AND attnum > 0 AND NOT attisdropped", view_name.replace("\"", "\"\""));
    let all_cols: Vec<String> = Spi::connect(|client| {
        Ok::<Vec<String>, spi::Error>(
            client
                .select(&cols_query, None, &[])?
                .map(|r| r.get::<String>(1).unwrap().unwrap())
                .collect(),
        )
    })
    .unwrap_or_default();

    if all_cols.is_empty() {
        return false;
    }

    let update_cols: Vec<String> = all_cols
        .iter()
        .filter(|&c| c != "t" && !scope_cols_raw.contains(c))
        .map(|c| format!("\"{}\"", c))
        .collect();

    let (sql_child, _) =
        rollup::derive_child_sql(view_name, &source_table, frame_seconds, &scope_cols_raw);
    if sql_child.is_empty() {
        return false;
    }
    let select_part = sql_child
        .split("SELECT")
        .nth(1)
        .unwrap()
        .split("FROM")
        .next()
        .unwrap()
        .trim();

    let mut on_clause = vec!["target.t::timestamptz = source.t::timestamptz".to_string()];
    for col in &scope_cols {
        on_clause.push(format!("target.{} = source.{}", col, col));
    }
    let update_set = update_cols
        .iter()
        .map(|c| format!("{} = source.{}", c, c))
        .collect::<Vec<_>>()
        .join(", ");

    let mut source_where = if scope_cols_raw.is_empty() {
        format!(
            "JOIN spiral.changelog c ON c.base_view = '{changelog_key}'
                AND spiral(t) >= (c.t_start/{0})*{0}
                AND spiral(t) < ((c.t_end/{0})+1)*{0}
                AND c.scope_values = '{{}}'::jsonb",
            frame_seconds,
            changelog_key = changelog_key.replace("'", "''")
        )
    } else {
        let scope_cols_json = scope_cols_raw
            .iter()
            .map(|s| format!("'{}', \"{}\"::text", s, s))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "JOIN spiral.changelog c ON c.base_view = '{changelog_key}'
                AND spiral(t) >= (c.t_start/{0})*{0}
                AND spiral(t) < ((c.t_end/{0})+1)*{0}
                AND (c.scope_values = '{{}}'::jsonb OR c.scope_values = jsonb_build_object({1}))",
            frame_seconds,
            scope_cols_json,
            changelog_key = changelog_key.replace("'", "''")
        )
    };
    if let Some(ref extra) = extra_where {
        source_where.push_str(&format!(" WHERE ({})", extra));
    }

    let group_by_clause = if scope_cols.is_empty() {
        "1".to_string()
    } else {
        format!("1, {}", scope_cols.join(", "))
    };

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
        view_name = view_name,
        select_part = select_part,
        source_table = source_table,
        source_where = source_where,
        group_by_clause = group_by_clause,
        on_clause = on_clause.join(" AND "),
        update_set = if update_set.is_empty() {
            "t = source.t"
        } else {
            &update_set
        },
        all_cols_joined = all_cols
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", "),
        source_cols_joined = all_cols
            .iter()
            .map(|c| format!("source.\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", ")
    );

    SKIP_ACCELERATION.with(|s| s.set(true));
    let _ = Spi::run(&merge_sql);
    SKIP_ACCELERATION.with(|s| s.set(false));

    let children = catalog::get_children(view_name);
    for child in children {
        let _ = refresh_incremental(&child, extra_where.clone(), depth + 1);
    }
    true
}

#[pg_extern]
fn spiral_to_epoch(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    spiral(t)
}

#[pg_extern]
fn spiral_from_epoch(epoch: i64) -> pgrx::datum::TimestampWithTimeZone {
    let micros = (epoch - 946684800) * 1000000;
    unsafe {
        pgrx::datum::TimestampWithTimeZone::from_datum(micros.into_datum().unwrap(), false).unwrap()
    }
}

#[pg_extern]
fn spiral_purge(base_table: &str) {
    let _ = Spi::run(&format!(
        "DELETE FROM spiral.changelog WHERE base_view = '{}'",
        base_table.replace("'", "''")
    ));
    notice!("Spiral: Changelog purged for '{}'", base_table);
}

#[pg_extern]
fn spiral_status(base_table: &str) -> pgrx::JsonB {
    Spi::connect(|client| {
        let mut status = serde_json::Map::new();

        let mut views = Vec::new();
        let table = client.select("SELECT view_name, frame_seconds, parent_view FROM spiral.metadata WHERE base_view = $1 ORDER BY frame_seconds ASC", None,
            unsafe { &[pgrx::datum::DatumWithOid::new(base_table.into_datum().unwrap(), pg_sys::TEXTOID)] })?;

        for row in table {
            let mut v_info = serde_json::Map::new();
            let v_name = row.get::<String>(1)?.unwrap();
            v_info.insert("frame_seconds".to_string(), serde_json::Value::from(row.get::<i32>(2)?.unwrap()));
            v_info.insert("parent".to_string(), serde_json::Value::from(row.get::<String>(3)?.unwrap()));

            let count = client.select(&format!("SELECT count(*) FROM \"{}\"", v_name.replace("\"", "\"\"")), Some(1), &[])?.get_one::<i64>()?.unwrap_or(0);
            v_info.insert("row_count".to_string(), serde_json::Value::from(count));

            views.push(serde_json::Value::Object(v_info));
        }
        status.insert("hierarchy".to_string(), serde_json::Value::Array(views));

        let dirty_count = client.select("SELECT count(*) FROM spiral.changelog WHERE base_view = $1", Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(base_table.into_datum().unwrap(), pg_sys::TEXTOID)] })?.get_one::<i64>()?.unwrap_or(0);
        status.insert("dirty_segments_count".to_string(), serde_json::Value::from(dirty_count));

        Ok::<pgrx::JsonB, spi::Error>(pgrx::JsonB(serde_json::Value::Object(status)))
    }).unwrap()
}

#[pg_extern(name = "spiral_refresh")]
fn spiral_refresh(view_name: &str, where_clause: default!(Option<&str>, "NULL")) {
    hooks::reactive_refresh(view_name, where_clause.map(|s| s.to_string()));
}

#[pg_extern(name = "spiral_register_view_rust")]
fn spiral_register_view(
    view_name: &str,
    parent_view: &str,
    frame_seconds: i32,
    base_view: &str,
    scope_columns: Vec<String>,
) {
    notice!(
        "Spiral: registering view '{}', parent='{}', base='{}'",
        view_name,
        parent_view,
        base_view
    );

    let table_exists = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = $1)",
        &[unsafe {
            pgrx::datum::DatumWithOid::new(view_name.into_datum().unwrap(), pg_sys::TEXTOID)
        }],
    )
    .unwrap()
    .unwrap_or(false);

    let source_table = if parent_view == "BASE" {
        base_view
    } else {
        parent_view
    };
    let (sql, sources) =
        rollup::derive_child_sql(view_name, source_table, frame_seconds, &scope_columns);

    if !table_exists && !sql.is_empty() {
        let create_sql = format!(
            "CREATE TABLE IF NOT EXISTS {} AS SELECT * FROM ({}) s LIMIT 0",
            view_name,
            sql.split(" AS ").nth(1).unwrap().split(';').next().unwrap()
        );
        let _ = Spi::run(&create_sql);
    }

    catalog::insert_metadata(
        view_name,
        parent_view,
        frame_seconds,
        base_view,
        scope_columns.clone(),
        pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
    );
    notice!("Spiral: view '{}' metadata inserted", view_name);
    for src in sources {
        catalog::insert_source(
            view_name,
            base_view,
            frame_seconds,
            &src.base_column,
            &src.formula,
            &src.mat_column,
            pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
        );
    }
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
                "CREATE TRIGGER spiral_track_{base_view}_{event_lower}
                 AFTER {event} ON \"{base_view}\"
                 {transition}
                 FOR EACH STATEMENT EXECUTE FUNCTION spiral.track_changes_stmt('{base_view}')",
                base_view = base_view,
                event = event,
                event_lower = event.to_lowercase(),
                transition = transition
            );
            let _ = Spi::run(&trigger_sql);
        }
    }
    notice!("Spiral: view '{}' registration complete", view_name);
}

pub fn get_kickoff_epoch() -> i64 {
    Spi::get_one::<i64>("SELECT spiral(COALESCE(current_setting('spiral.kickoff_date', true), '1970-01-01')::timestamptz)").unwrap_or(Some(0)).unwrap()
}

pub fn get_minimal_pace() -> f64 {
    Spi::get_one::<f64>(
        "SELECT COALESCE(current_setting('spiral.minimal_pace', true), '60')::numeric::float8",
    )
    .unwrap_or(Some(60.0))
    .unwrap()
}

#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}
    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        vec!["shared_preload_libraries = 'spiral'"]
    }
}
