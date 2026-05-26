use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;
use std::cell::Cell;

pub mod bgworker;
pub mod catalog;
pub mod hooks;
pub mod rollup;
pub mod stats;
pub mod storage;
pub mod tam;
pub mod validate;
pub mod zorder;
pub use zorder::*;

pgrx::pg_module_magic!();

extension_sql_file!("../sql/spiral.sql", name = "spiral_setup");

pub static WORKER_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static WORKER_DEBUG: GucSetting<bool> = GucSetting::<bool>::new(false);
pub static WORKER_MAX: GucSetting<i32> = GucSetting::<i32>::new(1);
pub static WORKER_BATCH_SIZE: GucSetting<i32> = GucSetting::<i32>::new(10);
pub static ENABLE_PLANNER_HOOK: GucSetting<bool> = GucSetting::<bool>::new(true);

thread_local! {
    pub static SKIP_ACCELERATION: Cell<bool> = const { Cell::new(false) };
    pub static SCAN_TIME_RANGE: Cell<Option<(i64, i64)>> = const { Cell::new(None) };
}

#[pg_guard]
pub unsafe extern "C-unwind" fn _PG_init() {
    hooks::init_hooks();

    GucRegistry::define_bool_guc(
        c"spiral.worker_enabled",
        c"Enable or disable the autonomous background worker",
        c"When true, the background worker will periodically refresh dirty segments.",
        &WORKER_ENABLED,
        GucContext::Sighup,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"spiral.worker_debug",
        c"Enable debug logging for the autonomous background worker",
        c"When true, the background worker will emit debug2 logs.",
        &WORKER_DEBUG,
        GucContext::Sighup,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"spiral.max_workers",
        c"Maximum number of parallel background workers",
        c"Caps the number of workers that can refresh materialized views concurrently.",
        &WORKER_MAX,
        1,
        100,
        GucContext::Sighup,
        GucFlags::default(),
    );

    GucRegistry::define_int_guc(
        c"spiral.worker_batch_size",
        c"Max scopes refreshed per worker tick",
        c"Each worker processes at most this many (base_view, scope) pairs per 1-second tick.",
        &WORKER_BATCH_SIZE,
        1,
        1000,
        GucContext::Sighup,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"spiral.enable_planner_hook",
        c"Enable or disable the Spiral planner hook",
        c"When false, standard PostgreSQL planning path is used without Spiral-specific optimizations.",
        &ENABLE_PLANNER_HOOK,
        GucContext::Userset,
        GucFlags::default(),
    );
}

#[pg_extern]
fn spiral_is_loaded() -> bool {
    true
}

const POSTGRES_EPOCH_JDATE: i64 = 946684800;

#[pg_extern(immutable, parallel_safe)]
fn spiral(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    let micros = unsafe { i64::from_datum(t.into_datum().unwrap(), false).unwrap() };
    (micros / 1000000) + POSTGRES_EPOCH_JDATE
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
fn scope_json_to_where(scope_json: &str) -> Option<String> {
    let val: serde_json::Value = serde_json::from_str(scope_json).ok()?;
    let obj = val.as_object()?;
    if obj.is_empty() {
        return None;
    }
    let clauses: Vec<String> = obj
        .iter()
        .map(|(k, v)| {
            let rhs = match v {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => format!("'{}'", v.to_string().replace('\'', "''")),
            };
            format!("\"{}\" = {}", k.replace('"', "\"\""), rhs)
        })
        .collect();
    Some(clauses.join(" AND "))
}

fn refresh_incremental(
    view_name: &str,
    extra_where: Option<String>,
    depth: i32,
    scope_json: Option<String>,
) -> bool {
    if depth > 5 {
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
        let res = Spi::get_one::<String>(&format!(
            "SELECT base_view FROM spiral.metadata WHERE view_name = '{}'",
            view_name.replace("'", "''")
        ));

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

    if frame_seconds > 0 {
        let (sql_child, _) = rollup::derive_child_sql(
            view_name,
            &source_table,
            frame_seconds,
            &scope_cols_raw,
            rollup::calendar_field_for_seconds(frame_seconds),
        );
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

        let safe_key = changelog_key.replace('\'', "''");
        let mut source_where = if let Some(ref sj) = scope_json {
            let safe_sj = sj.replace('\'', "''");
            format!(
                "JOIN spiral.changelog c ON c.base_view = '{safe_key}'
                    AND spiral(t) >= (c.t_start/{0})*{0}
                    AND spiral(t) < ((c.t_end/{0})+1)*{0}
                    AND c.scope_values = '{safe_sj}'::jsonb",
                frame_seconds,
            )
        } else if scope_cols_raw.is_empty() {
            format!(
                "JOIN spiral.changelog c ON c.base_view = '{safe_key}'
                    AND spiral(t) >= (c.t_start/{0})*{0}
                    AND spiral(t) < ((c.t_end/{0})+1)*{0}
                    AND c.scope_values = '{{}}'::jsonb",
                frame_seconds,
            )
        } else {
            let scope_cols_json = scope_cols_raw
                .iter()
                .map(|s| format!("'{}', \"{}\"", s, s))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "JOIN spiral.changelog c ON c.base_view = '{safe_key}'
                    AND spiral(t) >= (c.t_start/{0})*{0}
                    AND spiral(t) < ((c.t_end/{0})+1)*{0}
                    AND (c.scope_values = '{{}}'::jsonb OR c.scope_values = jsonb_build_object({1}))",
                frame_seconds,
                scope_cols_json,
            )
        };

        let base_where = scope_json
            .as_deref()
            .and_then(scope_json_to_where)
            .or_else(|| extra_where.clone());
        if let Some(ref w) = base_where {
            source_where.push_str(&format!(" WHERE ({})", w));
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
    }

    let children = catalog::get_children(view_name);
    for child in children {
        if child != view_name {
            let _ = refresh_incremental(&child, extra_where.clone(), depth + 1, scope_json.clone());
        }
    }
    true
}

#[pg_extern]
fn spiral_to_epoch(t: pgrx::datum::TimestampWithTimeZone) -> i64 {
    spiral(t)
}

#[pg_extern]
fn spiral_from_epoch(epoch: i64) -> pgrx::datum::TimestampWithTimeZone {
    let micros = (epoch - POSTGRES_EPOCH_JDATE) * 1000000;
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
}

#[pg_extern]
fn spiral_status(base_table: &str) -> pgrx::JsonB {
    Spi::connect(|client| {
        let mut status = serde_json::Map::new();

        let mut views = Vec::new();
        let table = client.select(&format!("SELECT view_name, frame_seconds, parent_view FROM spiral.metadata WHERE base_view = '{}' ORDER BY frame_seconds ASC", base_table.replace("'", "''")), None, &[])?;

        for row in table {
            let mut v_info = serde_json::Map::new();
            let v_name = row.get::<String>(1)?.unwrap_or_default();
            v_info.insert("frame_seconds".to_string(), serde_json::Value::from(row.get::<i32>(2)?.unwrap_or(0)));
            v_info.insert("parent".to_string(), serde_json::Value::from(row.get::<String>(3)?.unwrap_or_default()));

            if !v_name.is_empty() {
                let count_query = format!("SELECT count(*) FROM \"{}\"", v_name.replace("\"", "\"\""));
                let count = client.select(&count_query, Some(1), &[])?.get_one::<i64>()?.unwrap_or(0);
                v_info.insert("row_count".to_string(), serde_json::Value::from(count));
            }

            views.push(serde_json::Value::Object(v_info));
        }
        status.insert("hierarchy".to_string(), serde_json::Value::Array(views));

        let dirty_count_res = client.select(&format!("SELECT count(*) FROM spiral.changelog WHERE base_view = '{}'", base_table.replace("'", "''")), Some(1), &[]);

        let dirty_count = match dirty_count_res {
            Ok(t) => t.get_one::<i64>().unwrap_or(Some(0)).unwrap_or(0),
            Err(_) => 0,
        };
        status.insert("dirty_segments_count".to_string(), serde_json::Value::from(dirty_count));

        Ok::<pgrx::JsonB, spi::Error>(pgrx::JsonB(serde_json::Value::Object(status)))
    }).unwrap_or_else(|e| {
        let mut err_map = serde_json::Map::new();
        err_map.insert("error".to_string(), serde_json::Value::String(format!("{:?}", e)));
        pgrx::JsonB(serde_json::Value::Object(err_map))
    })
}

#[pg_extern(name = "spiral_refresh")]
fn spiral_refresh(view_name: &str, where_clause: default!(Option<&str>, "NULL")) {
    hooks::reactive_refresh(view_name, where_clause.map(|s| s.to_string()));
}

#[pg_extern(name = "spiral_refresh_scope")]
fn spiral_refresh_scope(view_name: &str, scope_json: &str) {
    hooks::reactive_refresh_by_scope(view_name, scope_json.to_string());
}

#[pg_extern(name = "spiral_register_view_rust")]
fn spiral_register_view(
    view_name: &str,
    parent_view: &str,
    frame_seconds: i32,
    base_view: &str,
    scope_columns: Vec<String>,
) {
    let table_exists = Spi::get_one::<bool>(&format!(
        "SELECT EXISTS (SELECT 1 FROM pg_class WHERE relname = '{}')",
        view_name.replace("'", "''")
    ))
    .unwrap_or(Some(false))
    .unwrap_or(false);

    let source_table = if parent_view == "BASE" {
        base_view
    } else {
        parent_view
    };
    let (sql, sources) = rollup::derive_child_sql(
        view_name,
        source_table,
        frame_seconds,
        &scope_columns,
        rollup::calendar_field_for_seconds(frame_seconds),
    );

    if !table_exists && !sql.is_empty() {
        let select_part = if let Some(part) = sql.split_once(" AS").map(|x| x.1) {
            let trimmed = part.trim();
            trimmed.split(';').next().unwrap_or("")
        } else {
            ""
        };

        if !select_part.is_empty() {
            let scope_cols_str = scope_columns
                .iter()
                .map(|s| format!("\"{}\"", s.trim()))
                .collect::<Vec<_>>()
                .join(", ");
            let index_sql = if scope_columns.is_empty() {
                format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{view_name} ON {view_name}(t)")
            } else {
                format!(
                    "CREATE INDEX IF NOT EXISTS idx_z_{view_name} ON {view_name} \
                     (spiral_zorder(spiral(t), ARRAY[{scope_cols_str}]::text[]))"
                )
            };
            let create_table_sql = format!(
                "CREATE TABLE IF NOT EXISTS {} AS SELECT * FROM ({}) s LIMIT 0",
                view_name, select_part
            );
            let _ = Spi::run(&create_table_sql);
            let _ = Spi::run(&index_sql);
        }
    }

    if view_name != base_view {
        catalog::insert_metadata(
            base_view,
            "BASE",
            0,
            base_view,
            scope_columns.clone(),
            pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
        );
    }
    catalog::insert_metadata(
        view_name,
        parent_view,
        frame_seconds,
        base_view,
        scope_columns.clone(),
        pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
    );
    for src in sources {
        catalog::insert_source(
            view_name,
            base_view,
            frame_seconds,
            &src.base_column,
            &src.formula,
            &src.mat_column,
            src.rollup_gsub_strategy.as_deref(),
            pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new())),
        );
    }
    if parent_view == "BASE" || parent_view == base_view {
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
                "CREATE OR REPLACE TRIGGER spiral_track_{base_view}_{event_lower}
                 AFTER {event} ON \"{base_view}\"
                 {transition}
                 FOR EACH STATEMENT EXECUTE FUNCTION spiral.track_changes_stmt('{base_view}', '{frame_seconds}')",
                base_view = base_view,
                event = event,
                event_lower = event.to_lowercase(),
                transition = transition,
                frame_seconds = frame_seconds
            );
            let _ = Spi::run(&trigger_sql);
        }
    }
}

pub fn get_kickoff_epoch() -> i64 {
    Spi::get_one::<i64>("SELECT spiral(COALESCE(NULLIF(current_setting('spiral.kickoff_date', true), ''), '2000-01-01')::timestamptz)").unwrap_or(Some(0)).unwrap_or(0)
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

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use crate::catalog;
    use pgrx::prelude::*;

    #[pg_test]
    fn test_pg_framework() {
        let result = pgrx::Spi::get_one::<i32>("SELECT 1 + 1");
        assert_eq!(result, Ok(Some(2)));
    }

    #[pg_test]
    fn test_catalog_is_spiral_relation() {
        assert!(!catalog::is_spiral_relation("non_existent_table"));
        catalog::insert_metadata(
            "test_view",
            "parent_view",
            60,
            "base_view",
            vec!["col1".to_string()],
            pgrx::JsonB(serde_json::json!({})),
        );
        assert!(catalog::is_spiral_relation("test_view"));
    }
}

#[cfg(test)]
mod zorder_tests {
    use super::*;

    #[test]
    fn test_zorder_zero() {
        assert_eq!(spiral_zorder_core(0, vec![]), 0);
    }

    #[test]
    fn test_zorder_int_array_basic() {
        assert_eq!(spiral_zorder_int_array_core(1, vec![1]), 3);
    }

    #[test]
    fn test_zorder_3d_basic() {
        assert_eq!(zorder_3d_core(1, 1, 1), 7);
        assert_eq!(zorder_3d_core(2, 0, 0), 8);
    }

    #[test]
    fn test_zorder_large_timestamp_does_not_wrap() {
        let t_over_u32 = u32::MAX as i64 + 1;
        assert_ne!(
            spiral_zorder_int_array_core(t_over_u32, vec![0]),
            spiral_zorder_int_array_core(0, vec![0])
        );
    }

    #[test]
    fn test_zorder_no_collision_within_full_range() {
        let r1 = spiral_zorder_int_array_core(1 << 16, vec![0]);
        let r2 = spiral_zorder_int_array_core((1 << 16) + 1, vec![0]);
        assert_ne!(r1, r2);
        let r3 = spiral_zorder_int_array_core(1_i64 << 32, vec![0]);
        assert_ne!(r3, spiral_zorder_int_array_core(0, vec![0]));
    }

    #[test]
    fn test_zorder_ordering_preserved() {
        let results: Vec<u128> = (0..4)
            .map(|t| spiral_zorder_int_array_core(t, vec![0]))
            .collect();
        for w in results.windows(2) {
            assert!(w[0] < w[1]);
        }
    }

    #[test]
    fn test_zorder_3d_uses_42_bits() {
        let x = 1i64 << 41;
        let result = zorder_3d_core(x, 0, 0);
        let expected: u128 = 1u128 << 123;
        assert_eq!(result, expected);
    }

    #[test]
    fn test_zorder_string_hash_stable() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"sensor_a"), 11_461_482_958_241_160_951_u64);
        let za = spiral_zorder_core(0, vec![Some("tenant_a".to_string())]);
        let zb = spiral_zorder_core(0, vec![Some("tenant_b".to_string())]);
        assert_ne!(za, zb);
    }

    #[test]
    fn test_zorder_monotone_for_fixed_string_dimension() {
        let tenant = Some("42".to_string());
        let timestamps: Vec<i64> =
            vec![0, 1, 100, 1000, 86400, 2_000_000, i32::MAX as i64, i64::MAX];
        let zorders: Vec<u128> = timestamps
            .iter()
            .map(|&t| spiral_zorder_core(t, vec![tenant.clone()]))
            .collect();
        for w in zorders.windows(2) {
            assert!(w[0] < w[1]);
        }
    }
}
