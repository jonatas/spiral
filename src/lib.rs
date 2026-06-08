#![allow(
    clippy::too_many_arguments,
    clippy::unnecessary_unwrap,
    clippy::nonminimal_bool,
    clippy::missing_safety_doc,
    clippy::needless_range_loop,
    clippy::field_reassign_with_default,
    clippy::from_str_radix_10,
    dead_code,
    unused_parens
)]

use pgrx::guc::{GucContext, GucFlags, GucRegistry, GucSetting};
use pgrx::prelude::*;
use std::cell::Cell;

pub mod bgworker;
pub mod catalog;
pub mod hooks;
pub mod mvcc;
pub mod rollup;
pub mod stats;
pub mod storage;
pub mod tam;
pub mod validate;
pub mod zorder;

pgrx::pg_module_magic!();

extension_sql_file!("../sql/spiral.sql", name = "spiral_setup");

pub static WORKER_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static WORKER_DEBUG: GucSetting<bool> = GucSetting::<bool>::new(false);
pub static WORKER_MAX: GucSetting<i32> = GucSetting::<i32>::new(1);
pub static WORKER_BATCH_SIZE: GucSetting<i32> = GucSetting::<i32>::new(100);
pub static ENABLE_PLANNER_HOOK: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static PLANNER_MAX_SEGMENTS: GucSetting<i32> = GucSetting::<i32>::new(100);
pub static KICKOFF_DATE: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(None);
pub static MINIMAL_PACE: GucSetting<f64> = GucSetting::<f64>::new(60.0);
/// When true (default), a WARNING is emitted on the first Spiral TAM write
/// per session to indicate that MVCC / rollback semantics are absent.
pub static TAM_WARN_WRITES: GucSetting<bool> = GucSetting::<bool>::new(true);

thread_local! {
    pub static SKIP_ACCELERATION: Cell<bool> = const { Cell::new(false) };
    /// Set by the planner hook when a time-range constraint is detected on a spiral
    /// table that will be scanned via the TAM (not redirected to a rollup tier).
    /// Consumed (taken) by `spiral_scan_begin` to compute the first/last page.
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

    GucRegistry::define_int_guc(
        c"spiral.planner_max_segments",
        c"Maximum segments for hierarchical UNION ALL before falling back to RAW",
        c"Limits plan complexity by choosing RAW scan when rollups are too fragmented.",
        &PLANNER_MAX_SEGMENTS,
        0,
        10000,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_string_guc(
        c"spiral.kickoff_date",
        c"Base date for time-relative indexing",
        c"Timestamps are stored as offsets from this date. Defaults to 2000-01-01.",
        &KICKOFF_DATE,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_float_guc(
        c"spiral.minimal_pace",
        c"Minimum seconds per bucket for dirty tracking",
        c"Smaller values increase precision but also changelog churn.",
        &MINIMAL_PACE,
        1.0,
        3600.0 * 24.0,
        GucContext::Userset,
        GucFlags::default(),
    );

    GucRegistry::define_bool_guc(
        c"spiral.warn_on_tam_writes",
        c"Warn on first Spiral TAM write per session (non-ACID)",
        c"Spiral TAM lacks MVCC: writes are not rolled back on ROLLBACK, and \
          snapshot isolation is absent. When true (default), a WARNING is emitted \
          on the first write per session. Set to false to suppress after \
          acknowledging the limitation.",
        &TAM_WARN_WRITES,
        GucContext::Userset,
        GucFlags::default(),
    );
}

#[pg_extern]
fn spiral_is_loaded() -> bool {
    true
}

pub const POSTGRES_EPOCH_JDATE: i64 = 946684800; // seconds between 1970-01-01 and 2000-01-01

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
/// Convert a scope_values JSONB string like `{"tenant_id": 42}` to a SQL WHERE clause
/// like `"tenant_id" = 42` for tight base-table index pushdown during per-scope refresh.
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

    let cols_query = format!("SELECT attname::text FROM pg_attribute WHERE attrelid = to_regclass('\"{}\"') AND attnum > 0 AND NOT attisdropped", view_name.replace("\"", "\"\""));
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

        let safe_key = changelog_key.replace('\'', "''");
        let (changelog_scope_match, target_scope_where) = if let Some(ref sj) = scope_json {
            let safe_sj = sj.replace('\'', "''");
            let tsw = scope_json_to_where(sj).unwrap_or_else(|| "1=1".to_string());
            (format!("c.scope_values = '{}'::jsonb", safe_sj), tsw)
        } else if scope_cols_raw.is_empty() {
            (
                "c.scope_values = '{}'::jsonb".to_string(),
                "1=1".to_string(),
            )
        } else {
            let scope_cols_json = scope_cols_raw
                .iter()
                .map(|s| format!("'{}', target.\"{}\"", s, s))
                .collect::<Vec<_>>()
                .join(", ");
            (
                format!(
                    "(c.scope_values = '{{}}'::jsonb OR c.scope_values = jsonb_build_object({}))",
                    scope_cols_json
                ),
                "1=1".to_string(),
            )
        };

        let group_by_clause = if scope_cols.is_empty() {
            "1".to_string()
        } else {
            format!("1, {}", scope_cols.join(", "))
        };

        let base_where = scope_json
            .as_deref()
            .and_then(scope_json_to_where)
            .or_else(|| extra_where.clone());

        let base_metadata_owned = catalog::get_metadata(&metadata.base_view);
        let source_time_col = if parent_view == "BASE" {
            base_metadata_owned
                .as_ref()
                .and_then(|m| {
                    m.columns_metadata
                        .get("time_column")
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("t")
        } else {
            "t"
        };

        let source_changelog_scope_match = if let Some(ref sj) = scope_json {
            let safe_sj = sj.replace('\'', "''");
            format!("c.scope_values = '{}'::jsonb", safe_sj)
        } else if scope_cols_raw.is_empty() {
            "c.scope_values = '{}'::jsonb".to_string()
        } else {
            let scope_cols_json = scope_cols_raw
                .iter()
                .map(|s| format!("'{}', s.\"{}\"", s, s))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "(c.scope_values = '{{}}'::jsonb OR c.scope_values = jsonb_build_object({}))",
                scope_cols_json
            )
        };

        let all_cols_joined = all_cols
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", ");

        // Conflict target: (scope_cols..., t) — matches the idx_u_* unique index.
        let conflict_cols_str = {
            let mut cols = scope_cols.clone();
            cols.push("\"t\"".to_string());
            cols.join(", ")
        };

        // Update set: all non-key columns (exclude t and scope cols).
        let update_set: String = all_cols
            .iter()
            .filter(|c| c.as_str() != "t" && !scope_cols_raw.contains(c))
            .map(|c| format!("\"{}\" = EXCLUDED.\"{}\"", c, c))
            .collect::<Vec<_>>()
            .join(", ");

        let on_conflict = if update_set.is_empty() {
            "ON CONFLICT DO NOTHING".to_string()
        } else {
            format!("ON CONFLICT ({conflict_cols_str}) DO UPDATE SET {update_set}")
        };

        // UPSERT: replaces the INSERT half of the old DELETE+INSERT.
        // Uses in-place HOT updates instead of creating dead tuples on every refresh cycle.
        let upsert_sql = format!(
            "INSERT INTO \"{view_name}\" ({all_cols_joined})
             SELECT {select_part} FROM \"{source_table}\" AS s
             WHERE EXISTS (
                 SELECT 1 FROM spiral.changelog c
                 WHERE c.base_view = '{safe_key}'
                   AND {source_changelog_scope_match}
                   AND spiral(s.\"{source_time_col}\") >= (c.t_start/{frame_seconds})*{frame_seconds}
                   AND spiral(s.\"{source_time_col}\") < c.t_end
             )
             {extra_filter}
             GROUP BY {group_by_clause}
             {on_conflict}",
            view_name = view_name,
            all_cols_joined = all_cols_joined,
            select_part = select_part,
            source_table = source_table,
            safe_key = safe_key,
            source_changelog_scope_match = source_changelog_scope_match,
            source_time_col = source_time_col,
            frame_seconds = frame_seconds,
            extra_filter = if let Some(ref w) = base_where { format!("AND ({})", w) } else { "".to_string() },
            group_by_clause = group_by_clause,
            on_conflict = on_conflict,
        );

        // Sparse DELETE: remove aggregate rows whose time bucket was touched by the changelog
        // but now has no source data (handles source-row deletion).
        // Runs after the upsert so it only removes truly orphaned rows.
        let source_scope_match: String = scope_cols_raw
            .iter()
            .map(|sc| format!("AND s.\"{}\" = target.\"{}\"", sc, sc))
            .collect::<Vec<_>>()
            .join(" ");

        let sparse_delete_sql = format!(
            "DELETE FROM \"{view_name}\" AS target
             WHERE {target_scope_where}
               AND EXISTS (
                 SELECT 1 FROM spiral.changelog c
                 WHERE c.base_view = '{safe_key}'
                   AND {changelog_scope_match}
                   AND spiral(target.t) >= (c.t_start/{frame_seconds})*{frame_seconds}
                   AND spiral(target.t) < c.t_end
               )
               AND NOT EXISTS (
                 SELECT 1 FROM \"{source_table}\" AS s
                 WHERE (spiral(s.\"{source_time_col}\") / {frame_seconds}) * {frame_seconds} = spiral(target.t)
                 {source_scope_match}
               )",
            view_name = view_name,
            target_scope_where = target_scope_where,
            safe_key = safe_key,
            changelog_scope_match = changelog_scope_match,
            frame_seconds = frame_seconds,
            source_table = source_table,
            source_time_col = source_time_col,
            source_scope_match = source_scope_match,
        );

        SKIP_ACCELERATION.with(|s| s.set(true));
        let run_upsert = Spi::run(&upsert_sql);
        let run_result = if run_upsert.is_ok() {
            Spi::run(&sparse_delete_sql)
        } else {
            run_upsert
        };
        SKIP_ACCELERATION.with(|s| s.set(false));

        if let Err(e) = run_result {
            notice!(
                "Spiral: refresh_incremental failed for '{}': {:?}\nSQL: {}",
                view_name,
                e,
                upsert_sql
            );
            return false;
        }
    }

    let children = catalog::get_children(view_name);
    let mut all_ok = true;
    for child in children {
        if child != view_name
            && !refresh_incremental(&child, extra_where.clone(), depth + 1, scope_json.clone())
        {
            all_ok = false;
        }
    }
    all_ok
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
    notice!("Spiral: Changelog purged for '{}'", base_table);
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

/// Refresh a single scope identified by its scope_values JSONB text.
/// Building block for parallel dispatch: callers can invoke this concurrently
/// across scopes (e.g. via pg_background) to achieve N-scope parallelism.
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
    notice!(
        "Spiral: registering view '{}', parent='{}', base='{}'",
        view_name,
        parent_view,
        base_view
    );

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

    notice!("Spiral: derive_child_sql produced SQL: '{}'", sql);

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
            let create_table_sql = format!(
                "CREATE TABLE IF NOT EXISTS {} AS SELECT * FROM ({}) s LIMIT 0",
                view_name, select_part
            );
            let _ = Spi::run(&create_table_sql);
            let unique_sql = if scope_columns.is_empty() {
                format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{view_name} ON {view_name}(t)")
            } else {
                format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{view_name} ON {view_name}(t, {scope_cols_str})"
                )
            };
            let _ = Spi::run(&unique_sql);
            if !scope_columns.is_empty() {
                let zorder_sql = format!(
                    "CREATE INDEX IF NOT EXISTS idx_z_{view_name} ON {view_name} \
                     (spiral_zorder(spiral(t), ARRAY[{scope_cols_str}]::text[]))"
                );
                let _ = Spi::run(&zorder_sql);
            }
        }
    }

    let kickoff = get_kickoff_epoch();
    let mut cols_meta = serde_json::Map::new();
    cols_meta.insert(
        "kickoff_epoch".to_string(),
        serde_json::Value::Number(kickoff.into()),
    );

    // Ensure base table has a frame_seconds=0 metadata row so track_changes_stmt
    // can look up scope_columns via "WHERE view_name = base_view".
    if view_name != base_view {
        catalog::insert_metadata(
            base_view,
            "BASE",
            0,
            base_view,
            scope_columns.clone(),
            pgrx::JsonB(serde_json::Value::Object(cols_meta.clone())),
        );
    }
    catalog::insert_metadata(
        view_name,
        parent_view,
        frame_seconds,
        base_view,
        scope_columns.clone(),
        pgrx::JsonB(serde_json::Value::Object(cols_meta)),
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
    notice!("Spiral: view '{}' registration complete", view_name);
}

pub fn get_kickoff_epoch() -> i64 {
    let kickoff_str = Spi::get_one::<String>("SELECT current_setting('spiral.kickoff_date', true)")
        .unwrap_or(None)
        .unwrap_or_else(|| "2000-01-01".to_string());

    let kickoff_val = if kickoff_str.is_empty() {
        "2000-01-01".to_string()
    } else {
        kickoff_str
    };

    Spi::connect(|client| {
        let ts = client
            .select(
                &format!("SELECT '{}'::timestamptz", kickoff_val.replace("'", "''")),
                Some(1),
                &[],
            )?
            .first()
            .get::<pgrx::datum::TimestampWithTimeZone>(1)?
            .unwrap();
        Ok::<i64, spi::Error>(spiral_to_epoch(ts))
    })
    .unwrap_or(0)
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
    use crate::{catalog, hooks};
    use pgrx::prelude::*;

    #[pg_test]
    fn test_pg_framework() {
        // verify that pgrx spi connects and evaluates simple query
        let result = pgrx::Spi::get_one::<i32>("SELECT 1 + 1");
        assert_eq!(result, Ok(Some(2)));
    }

    #[pg_test]
    fn test_catalog_is_spiral_relation() {
        // Initially should be false for a random table
        assert!(!catalog::is_spiral_relation("non_existent_table"));

        // Let's insert some dummy metadata
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

    #[pg_test]
    fn test_catalog_get_children() {
        catalog::insert_metadata(
            "child1",
            "parent_view",
            60,
            "base_view",
            vec![],
            pgrx::JsonB(serde_json::json!({})),
        );
        catalog::insert_metadata(
            "child2",
            "parent_view",
            300,
            "base_view",
            vec![],
            pgrx::JsonB(serde_json::json!({})),
        );

        let children = catalog::get_children("parent_view");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0], "child1");
        assert_eq!(children[1], "child2");
    }

    #[pg_test]
    fn test_planner_supports_plain_sum_target_lists() {
        Spi::run(
            "CREATE TABLE planner_support (t timestamptz, tenant_id int, val double precision)",
        )
        .unwrap();

        unsafe {
            let query = hooks::parse_sql_to_query("SELECT sum(val) FROM planner_support");
            assert!(!query.is_null());

            let cols =
                hooks::extract_supported_query_columns(query, (*query).rtable, "planner_support");
            assert_eq!(
                cols,
                Some(vec![("val".to_string(), Some("sum".to_string()))])
            );
        }
    }

    #[pg_test]
    fn test_planner_supports_grouped_sum_target_lists() {
        Spi::run(
            "CREATE TABLE planner_grouped (t timestamptz, tenant_id int, val double precision)",
        )
        .unwrap();

        unsafe {
            let query = hooks::parse_sql_to_query(
                "SELECT sum(val), tenant_id FROM planner_grouped GROUP BY tenant_id",
            );
            assert!(!query.is_null());

            let cols =
                hooks::extract_supported_query_columns(query, (*query).rtable, "planner_grouped");
            assert_eq!(
                cols,
                Some(vec![("val".to_string(), Some("sum".to_string()))])
            );
        }
    }

    #[pg_test]
    fn test_planner_supports_date_trunc_group_by() {
        Spi::run(
            "CREATE TABLE planner_datetrunc (t timestamptz, user_id int, val double precision)",
        )
        .unwrap();

        unsafe {
            // date_trunc in target list should not block acceleration — T_FuncExpr is passthrough
            let query = hooks::parse_sql_to_query(
                "SELECT date_trunc('day', t), sum(val) FROM planner_datetrunc GROUP BY 1",
            );
            assert!(!query.is_null());

            let cols =
                hooks::extract_supported_query_columns(query, (*query).rtable, "planner_datetrunc");
            assert!(
                cols.is_some(),
                "date_trunc in target list should not block acceleration"
            );
            let cols = cols.unwrap();
            assert!(
                cols.iter()
                    .any(|(name, agg)| name == "val" && agg.as_deref() == Some("sum")),
                "sum(val) should be in cols: {:?}",
                cols
            );
        }
    }

    #[pg_test]
    fn test_monthly_rollup_calendar_alignment() {
        // Verify: 1M frame creates calendar-aligned month buckets (first-of-month),
        // not fixed 30-day epoch buckets.
        Spi::run("CREATE TABLE monthly_ticks (t timestamptz NOT NULL, val numeric)").unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('monthly_ticks', 'BASE', 0, 'monthly_ticks', '{}')").unwrap();
        // Register the monthly rollup view (frame_seconds = 2592000 = 30 days)
        Spi::run("SELECT spiral_register_view('monthly_ticks_1mon', 'monthly_ticks', 2592000, 'monthly_ticks', '{}')")
            .unwrap();

        // Use mid-month timestamps to avoid timezone-boundary ambiguity:
        // date_trunc('month', t) uses the server TZ; timestamps near midnight UTC
        // at month-start can appear in the prior month in negative-offset zones.
        Spi::run(
            "INSERT INTO monthly_ticks (t, val)
             VALUES
               ('2024-01-15 12:00:00+00'::timestamptz, 100),
               ('2024-02-10 08:00:00+00'::timestamptz, 200),
               ('2024-02-15 12:00:00+00'::timestamptz, 300),
               ('2024-03-15 12:00:00+00'::timestamptz, 400)",
        )
        .unwrap();

        Spi::run("SELECT spiral_refresh('monthly_ticks')").unwrap();

        // Rollup should have exactly 3 rows (Jan, Feb, Mar)
        let row_count: i64 = Spi::get_one("SELECT COUNT(*)::bigint FROM monthly_ticks_1mon")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(row_count, 3, "expected 3 monthly buckets (Jan, Feb, Mar)");

        // Each bucket t must be the first-of-month at the server's local timezone.
        // Use date_trunc('month', ...) to produce the expected value regardless of TZ.
        let jan_t: bool = Spi::get_one(
            "SELECT EXISTS(
               SELECT 1 FROM monthly_ticks_1mon
               WHERE t = date_trunc('month', '2024-01-15 12:00:00+00'::timestamptz)
             )",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(jan_t, "January bucket must be at first-of-month boundary");

        let feb_sum: Option<pgrx::AnyNumeric> = Spi::get_one(
            "SELECT val FROM monthly_ticks_1mon
             WHERE t = date_trunc('month', '2024-02-15 12:00:00+00'::timestamptz)",
        )
        .unwrap();
        assert!(feb_sum.is_some(), "February bucket must exist");

        let mar_sum: Option<pgrx::AnyNumeric> = Spi::get_one(
            "SELECT val FROM monthly_ticks_1mon
             WHERE t = date_trunc('month', '2024-03-15 12:00:00+00'::timestamptz)",
        )
        .unwrap();
        assert!(mar_sum.is_some(), "March bucket must exist");
    }

    #[pg_test]
    fn test_planner_rejects_unsupported_aggregate_target_lists() {
        Spi::run(
            "CREATE TABLE planner_fallback (t timestamptz, tenant_id int, val double precision)",
        )
        .unwrap();

        for sql in [
            "SELECT DISTINCT sum(val) FROM planner_fallback",
            "SELECT sum(val) FILTER (WHERE val > 0) FROM planner_fallback",
            "SELECT sum(val) FROM planner_fallback HAVING count(*) > 0",
        ] {
            unsafe {
                let query = hooks::parse_sql_to_query(sql);
                assert!(!query.is_null(), "expected parser output for {sql}");
                let cols = hooks::extract_supported_query_columns(
                    query,
                    (*query).rtable,
                    "planner_fallback",
                );
                assert!(cols.is_none(), "expected fallback for {sql}, got {cols:?}");
            }
        }
    }

    #[pg_test]
    fn test_multi_dim_group_by_acceleration() {
        // Planner must accelerate GROUP BY tenant_id, date_trunc('day', t)
        // routing to the rollup tier and producing correct per-tenant daily sums.
        Spi::run("CREATE TABLE mdim_ticks (t timestamptz NOT NULL, val numeric, tenant_id int4)")
            .unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('mdim_ticks', 'BASE', 0, 'mdim_ticks', '{tenant_id}')").unwrap();
        Spi::run("SELECT spiral_register_view('mdim_ticks_1h', 'mdim_ticks', 3600, 'mdim_ticks', '{tenant_id}')").unwrap();

        // Two tenants, two days each
        Spi::run(
            "INSERT INTO mdim_ticks (t, val, tenant_id) VALUES
             ('2024-01-01 01:00:00+00', 10, 1),
             ('2024-01-01 02:00:00+00', 20, 1),
             ('2024-01-02 01:00:00+00', 30, 1),
             ('2024-01-01 01:00:00+00', 100, 2),
             ('2024-01-02 01:00:00+00', 200, 2)",
        )
        .unwrap();

        Spi::run("SELECT spiral_refresh('mdim_ticks')").unwrap();

        // The rollup must have 4 rows: (tenant 1, day 1), (tenant 1, day 2),
        // (tenant 2, day 1), (tenant 2, day 2)
        let row_count: i64 = Spi::get_one("SELECT COUNT(*)::bigint FROM mdim_ticks_1h")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            row_count, 5,
            "expected 5 hourly rollup rows (2+1+1+1 hours across tenants)"
        );

        // Multi-dim GROUP BY via planner: should accelerate to rollup tier
        let t1_day1: Option<pgrx::AnyNumeric> = Spi::get_one(
            "SELECT SUM(val) FROM mdim_ticks
             WHERE tenant_id = 1 AND t BETWEEN '2024-01-01' AND '2024-01-01 23:59:59'
             GROUP BY tenant_id, date_trunc('day', t)
             LIMIT 1",
        )
        .unwrap();
        assert!(t1_day1.is_some(), "tenant 1 day 1 sum must be non-null");

        let t2_total: Option<pgrx::AnyNumeric> = Spi::get_one(
            "SELECT SUM(val) FROM mdim_ticks
             WHERE tenant_id = 2 AND t BETWEEN '2024-01-01' AND '2024-01-02 23:59:59'
             GROUP BY tenant_id
             LIMIT 1",
        )
        .unwrap();
        assert!(t2_total.is_some(), "tenant 2 total must be non-null");
    }

    #[pg_test]
    fn test_stats_accuracy_golden() {
        let values_csv = include_str!("../tests/golden/values.csv");
        let expected_json = include_str!("../tests/golden/expected.json");
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();

        let mut state = crate::stats::StatsState::default();
        for line in values_csv.lines() {
            if let Ok(v) = line.parse::<f64>() {
                state.add(v);
            }
        }

        let epsilon = 1e-9;
        assert!((state.mean() - expected["mean"].as_f64().unwrap()).abs() < epsilon);
        assert!((state.variance() - expected["variance"].as_f64().unwrap()).abs() < epsilon);
        assert!((state.skewness() - expected["skewness"].as_f64().unwrap()).abs() < epsilon);
        assert!((state.kurtosis() - expected["kurtosis"].as_f64().unwrap()).abs() < epsilon);
    }

    #[pg_test]
    fn test_zorder_correctness() {
        use crate::zorder::spiral_zorder_int_array;

        // Test case 1: 0, 0 -> 0
        assert_eq!(spiral_zorder_int_array(0, vec![0]).to_string(), "0");

        // Test case 2: 1, 0 -> 1 (Time bit 0 at position 0)
        assert_eq!(spiral_zorder_int_array(1, vec![0]).to_string(), "1");

        // Test case 3: 0, 1 -> 2 (Dimension bit 0 at position 1)
        assert_eq!(spiral_zorder_int_array(0, vec![1]).to_string(), "2");

        // Test case 4: 1, 1 -> 3 (Both bits 0 at positions 0 and 1)
        assert_eq!(spiral_zorder_int_array(1, vec![1]).to_string(), "3");

        // Test case 5: 2, 0 -> 4 (Time bit 1 at position 2)
        assert_eq!(spiral_zorder_int_array(2, vec![0]).to_string(), "4");

        // Test case 6: 0, 2 -> 8 (Dimension bit 1 at position 3)
        assert_eq!(spiral_zorder_int_array(0, vec![2]).to_string(), "8");
    }

    #[pg_test]
    fn test_hilbert_2d_correctness() {
        use crate::zorder::spiral_hilbert_2d;
        assert_eq!(spiral_hilbert_2d(0, 0).to_string(), "0");
        assert_eq!(spiral_hilbert_2d(1, 0).to_string(), "1");
        assert_eq!(spiral_hilbert_2d(1, 1).to_string(), "2");
        assert_eq!(spiral_hilbert_2d(0, 1).to_string(), "3");
    }

    #[pg_test]
    fn test_ivm_concurrent_write_safety() {
        // Use spiral_register_view (same pattern as test_spiral_validate_basic) to
        // avoid triggering maybe_start_worker via the WITH (spiral.frames) DDL path.
        Spi::run("CREATE TABLE cticks (t timestamptz NOT NULL, price numeric)").unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('cticks', 'BASE', 0, 'cticks', '{}')").unwrap();
        Spi::run("SELECT spiral_register_view('cticks_1h', 'cticks', 3600, 'cticks', '{}')")
            .unwrap();

        Spi::run(
            "INSERT INTO cticks (t, price)
             SELECT now() - interval '2 hours' + (i * interval '1 minute'), 100.0
             FROM generate_series(0, 5) i",
        )
        .unwrap();

        Spi::run("SELECT spiral_refresh('cticks')").unwrap();

        let initial: bool = Spi::get_one("SELECT COUNT(*) > 0 FROM cticks_1h")
            .unwrap()
            .unwrap_or(false);
        assert!(initial, "view should have rows after initial refresh");

        Spi::run(
            "INSERT INTO cticks (t, price)
             VALUES (now() - interval '2 hours' + interval '10 minutes', 999.0)",
        )
        .unwrap();

        let before: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'cticks'",
        )
        .unwrap()
        .unwrap_or(0);
        assert!(before > 0, "changelog should have entries before refresh");

        Spi::run("SELECT spiral_refresh('cticks')").unwrap();

        let after: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'cticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(after, 0, "changelog should be empty after refresh");

        let high_price: bool = Spi::get_one("SELECT MAX(price) >= 999 FROM cticks_1h")
            .unwrap()
            .unwrap_or(false);
        assert!(
            high_price,
            "refreshed view should contain injected price 999"
        );

        // Write B scenario: post-refresh insert must survive a second refresh.
        Spi::run(
            "INSERT INTO cticks (t, price)
             VALUES (now() - interval '2 hours' + interval '20 minutes', 111.0)",
        )
        .unwrap();
        Spi::run("SELECT spiral_refresh('cticks')").unwrap();

        Spi::run(
            "INSERT INTO cticks (t, price)
             VALUES (now() - interval '2 hours' + interval '25 minutes', 222.0)",
        )
        .unwrap();

        let pending: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'cticks'",
        )
        .unwrap()
        .unwrap_or(0);
        assert!(
            pending > 0,
            "write B should be pending in changelog before second refresh"
        );

        Spi::run("SELECT spiral_refresh('cticks')").unwrap();

        let write_b: bool = Spi::get_one("SELECT MAX(price) >= 222 FROM cticks_1h")
            .unwrap()
            .unwrap_or(false);
        assert!(
            write_b,
            "write B (price 222) must be present after second refresh"
        );
    }

    #[pg_test]
    fn test_refresh_noop_when_changelog_empty_and_rollup_populated() {
        // After a successful refresh clears the changelog, a second spiral_refresh
        // with no new inserts should be a no-op — it must NOT re-bootstrap with a
        // full-range entry and re-aggregate the entire history.
        Spi::run("CREATE TABLE ticks2 (t timestamptz NOT NULL, val numeric)").unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('ticks2', 'BASE', 0, 'ticks2', '{}')").unwrap();
        Spi::run("SELECT spiral_register_view('ticks2_1h', 'ticks2', 3600, 'ticks2', '{}')")
            .unwrap();

        Spi::run(
            "INSERT INTO ticks2 (t, val)
             SELECT now() - interval '3 hours' + (i * interval '10 minutes'), 42.0
             FROM generate_series(0, 5) i",
        )
        .unwrap();

        // Initial refresh: populates rollup and clears changelog.
        Spi::run("SELECT spiral_refresh('ticks2')").unwrap();

        let after_first: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'ticks2'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            after_first, 0,
            "changelog must be empty after initial refresh"
        );

        let rollup_rows: i64 = Spi::get_one("SELECT COUNT(*)::bigint FROM ticks2_1h")
            .unwrap()
            .unwrap_or(0);
        assert!(
            rollup_rows > 0,
            "rollup must have rows after initial refresh"
        );

        // Second refresh with no new data: changelog is empty, rollup is populated.
        // Must return immediately without re-bootstrapping.
        Spi::run("SELECT spiral_refresh('ticks2')").unwrap();

        let after_second: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'ticks2'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            after_second, 0,
            "changelog must remain empty after no-op refresh (no bootstrap re-trigger)"
        );

        let rollup_rows_after: i64 = Spi::get_one("SELECT COUNT(*)::bigint FROM ticks2_1h")
            .unwrap()
            .unwrap_or(0);
        assert_eq!(
            rollup_rows_after, rollup_rows,
            "rollup row count must be unchanged by no-op refresh"
        );
    }

    #[pg_test]
    fn test_partial_refresh_by_scope() {
        // spiral_refresh with a WHERE clause scoped to one tenant must only
        // process changelog entries for that tenant and leave others untouched.
        Spi::run("CREATE TABLE scope_ticks (t timestamptz NOT NULL, val numeric, tenant_id int4)")
            .unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('scope_ticks', 'BASE', 0, 'scope_ticks', '{tenant_id}')").unwrap();
        Spi::run("SELECT spiral_register_view('scope_ticks_1h', 'scope_ticks', 3600, 'scope_ticks', '{tenant_id}')").unwrap();

        // Insert data for two tenants
        Spi::run(
            "INSERT INTO scope_ticks (t, val, tenant_id)
             SELECT now() - interval '2 hours' + (i * interval '10 minutes'), i * 10.0, 1
             FROM generate_series(0, 5) i",
        )
        .unwrap();
        Spi::run(
            "INSERT INTO scope_ticks (t, val, tenant_id)
             SELECT now() - interval '2 hours' + (i * interval '10 minutes'), i * 20.0, 2
             FROM generate_series(0, 5) i",
        )
        .unwrap();

        // Verify both tenants produced changelog entries (via trigger)
        let total_entries: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'scope_ticks'",
        )
        .unwrap()
        .unwrap_or(0);
        assert!(
            total_entries >= 2,
            "both tenants should have changelog entries"
        );

        // Partial refresh for tenant 1 only
        Spi::run("SELECT spiral_refresh('scope_ticks', 'tenant_id = 1')").unwrap();

        // Tenant 2 entries must remain in changelog
        let remaining: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'scope_ticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert!(
            remaining > 0,
            "tenant 2 changelog entries must survive partial refresh of tenant 1"
        );

        // Tenant 1 rollup data must be present
        let t1_rows: i64 =
            Spi::get_one("SELECT COUNT(*)::bigint FROM scope_ticks_1h WHERE tenant_id = 1")
                .unwrap()
                .unwrap_or(0);
        assert!(
            t1_rows > 0,
            "rollup must have rows for tenant 1 after partial refresh"
        );

        // Tenant 2 rollup must be empty (not yet refreshed)
        let t2_rows: i64 =
            Spi::get_one("SELECT COUNT(*)::bigint FROM scope_ticks_1h WHERE tenant_id = 2")
                .unwrap()
                .unwrap_or(-1);
        assert_eq!(
            t2_rows, 0,
            "tenant 2 must not appear in rollup before its own refresh"
        );
    }

    #[pg_test]
    fn test_full_refresh_processes_all_scopes() {
        // Full spiral_refresh must process all tenant scopes and clear their changelog entries.
        // Per-scope MERGE must produce correct rollup rows for every distinct scope_values bucket.
        Spi::run("CREATE TABLE par_ticks (t timestamptz NOT NULL, val numeric, org_id int4)")
            .unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('par_ticks', 'BASE', 0, 'par_ticks', '{org_id}')").unwrap();
        Spi::run("SELECT spiral_register_view('par_ticks_1h', 'par_ticks', 3600, 'par_ticks', '{org_id}')").unwrap();

        // Insert 3 tenants, 4 rows each across 2 hours
        for org in 1..=3_i32 {
            Spi::run(&format!(
                "INSERT INTO par_ticks (t, val, org_id)
                 SELECT now() - interval '1 hour' + (i * interval '20 minutes'), {org} * 10.0, {org}
                 FROM generate_series(0, 3) i"
            ))
            .unwrap();
        }

        // All 3 tenants must have changelog entries
        let entries: i64 = Spi::get_one(
            "SELECT COUNT(DISTINCT scope_values)::bigint FROM spiral.changelog WHERE base_view = 'par_ticks'",
        )
        .unwrap()
        .unwrap_or(0);
        assert_eq!(entries, 3, "3 distinct scope buckets expected in changelog");

        // Full refresh — must process all scopes in one call
        Spi::run("SELECT spiral_refresh('par_ticks')").unwrap();

        // Changelog cleared
        let remaining: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'par_ticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(remaining, 0, "changelog must be empty after full refresh");

        // All 3 tenants have rollup rows
        for org in 1..=3_i32 {
            let rows: i64 = Spi::get_one(&format!(
                "SELECT COUNT(*)::bigint FROM par_ticks_1h WHERE org_id = {org}"
            ))
            .unwrap()
            .unwrap_or(0);
            assert!(
                rows > 0,
                "org {org} must have rollup rows after full refresh"
            );
        }
    }

    #[pg_test]
    fn test_spiral_refresh_scope_single_tenant() {
        // spiral_refresh_scope must refresh exactly one scope and leave others untouched.
        Spi::run("CREATE TABLE rscope_ticks (t timestamptz NOT NULL, val numeric, uid int4)")
            .unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('rscope_ticks', 'BASE', 0, 'rscope_ticks', '{uid}')").unwrap();
        Spi::run("SELECT spiral_register_view('rscope_ticks_1h', 'rscope_ticks', 3600, 'rscope_ticks', '{uid}')").unwrap();

        for uid in [10_i32, 20_i32] {
            Spi::run(&format!(
                "INSERT INTO rscope_ticks (t, val, uid)
                 SELECT now() - interval '30 minutes', {uid}.0, {uid}"
            ))
            .unwrap();
        }

        // Refresh only uid=10 via spiral_refresh_scope
        Spi::run("SELECT spiral_refresh_scope('rscope_ticks', '{\"uid\": 10}')").unwrap();

        let uid10_rows: i64 =
            Spi::get_one("SELECT COUNT(*)::bigint FROM rscope_ticks_1h WHERE uid = 10")
                .unwrap()
                .unwrap_or(0);
        assert!(uid10_rows > 0, "uid=10 must have rollup rows");

        let uid20_rows: i64 =
            Spi::get_one("SELECT COUNT(*)::bigint FROM rscope_ticks_1h WHERE uid = 20")
                .unwrap()
                .unwrap_or(0);
        assert_eq!(
            uid20_rows, 0,
            "uid=20 must be untouched by single-scope refresh"
        );

        // uid=20 changelog entry must still exist
        let remaining: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'rscope_ticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert!(remaining > 0, "uid=20 changelog entry must survive");
    }

    #[pg_test]
    fn test_hierarchy_build_failure_is_error_not_warning() {
        // A column annotated with an unsupported aggregate (sum on text) must
        // cause generate_hierarchy_internal to raise an ERROR, not silently
        // emit a warning and leave a broken hierarchy.
        // In pgrx's SPI test context the error surfaces as a Rust panic.
        let result = std::panic::catch_unwind(|| {
            Spi::run(
                "CREATE TABLE bad_hierarchy (
                    t       timestamptz NOT NULL,
                    label   text        -- Spiral: sum
                ) WITH (spiral.frames = '1h')",
            )
            .unwrap();
        });
        assert!(
            result.is_err(),
            "CREATE TABLE with un-aggregatable column must raise an error, not a silent warning"
        );
    }

    #[pg_test]
    fn test_utility_hook_resets_in_utility_after_bad_directive() {
        // Verify IN_UTILITY is false before we start.
        assert!(!hooks::is_in_utility_for_test());

        // Multi-word formula ("bad formula here") triggers error! inside the utility
        // hook body — after standard_ProcessUtility already ran (table exists).
        // Before the fix this left IN_UTILITY stuck true for the session.
        let result = std::panic::catch_unwind(|| {
            Spi::run(
                "CREATE TABLE _reentry_bad (
                    t timestamptz,
                    v float8  -- Spiral: bad formula here
                ) WITH (spiral.frames = '1h')",
            )
            .unwrap();
        });
        assert!(result.is_err(), "multi-word formula must raise an error");

        // PgTryBuilder.finally must have reset IN_UTILITY regardless of the error.
        assert!(
            !hooks::is_in_utility_for_test(),
            "IN_UTILITY stuck true after hook error — reentrancy guard broken"
        );
    }

    #[pg_test]
    fn test_planner_hook_resets_in_hook_after_normal_select() {
        // After the planner hook runs and returns normally, IN_HOOK must be false.
        // Verifies PgTryBuilder.finally fires on the happy path (replaces manual reset).
        Spi::run("CREATE TABLE _planner_reset (t timestamptz, v float8)").unwrap();
        let _ = Spi::get_one::<f64>("SELECT sum(v) FROM _planner_reset");
        assert!(
            !hooks::is_in_hook_for_test(),
            "IN_HOOK stuck true after planner hook completed — reentrancy guard broken"
        );
    }

    #[pg_test]
    fn test_hierarchy_build_success_has_triggers_and_rollup() {
        // Happy-path regression: a valid Spiral table must have changelog triggers
        // and at least one rollup tier created — no silent partial failures.
        Spi::run(
            "CREATE TABLE good_hierarchy (
                t   timestamptz NOT NULL,
                val double precision  -- Spiral: sum
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        let trigger_count: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM pg_trigger t
             JOIN pg_class c ON c.oid = t.tgrelid
             WHERE c.relname = 'good_hierarchy'",
        )
        .unwrap()
        .unwrap_or(0);
        assert!(
            trigger_count > 0,
            "changelog triggers must exist on the base table"
        );

        let rollup_exists: bool = Spi::get_one(
            "SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = 'good_hierarchy_1h')",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            rollup_exists,
            "rollup tier good_hierarchy_1h must be created"
        );
    }

    #[pg_test]
    fn test_magic_comment_type_mismatch_is_error() {
        // sum on a text column → numeric required → must raise an error.
        let result = std::panic::catch_unwind(|| {
            Spi::run(
                "CREATE TABLE type_mismatch (
                    t     timestamptz NOT NULL,
                    label text  -- Spiral: sum
                ) WITH (spiral.frames = '1h')",
            )
            .unwrap();
        });
        assert!(
            result.is_err(),
            "sum directive on text column must raise an error"
        );
    }

    #[pg_test]
    fn test_magic_comment_range_max_end_on_non_tstz_is_error() {
        // range_max_end on an integer column → timestamptz required → error.
        let result = std::panic::catch_unwind(|| {
            Spi::run(
                "CREATE TABLE tstz_mismatch (
                    t      timestamptz NOT NULL,
                    amount int  -- Spiral: range_max_end
                ) WITH (spiral.frames = '1h')",
            )
            .unwrap();
        });
        assert!(
            result.is_err(),
            "range_max_end directive on int column must raise an error"
        );
    }

    #[pg_test]
    fn test_magic_comment_prose_false_positive_ignored() {
        // A standalone "-- Note: the Spiral: sum ..." comment must NOT be captured
        // as a column directive. The same-line anchor ([ \t]* not \s*) prevents
        // the regex from matching across newlines, and the column-existence filter
        // rejects tokens that are not real column names.
        // Verification: CREATE TABLE succeeds (no phantom error from prose comment).
        Spi::run(
            "CREATE TABLE prose_comment (
                t   timestamptz NOT NULL,
                val double precision
                -- Note: the Spiral: sum formula is documented elsewhere
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        let rollup_exists: bool = Spi::get_one::<bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = 'prose_comment_1h')",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            rollup_exists,
            "rollup tier must be created even when a prose comment mentions Spiral:"
        );
    }

    #[pg_test]
    fn test_sparse_bulk_insert_produces_bucketed_changelog() {
        // Inserting N rows spread over a wide time span must create N changelog
        // entries (one per frame-bucket touched), NOT a single amplified span.
        // This verifies that dirty-range amplification is eliminated.
        Spi::run(
            "CREATE TABLE sparse_events (
                t   timestamptz NOT NULL,
                val double precision
            ) WITH (spiral.frames = '1h,1d')",
        )
        .unwrap();

        // 10 rows, each 1 day apart (10-day span, 1h buckets → 10 distinct buckets).
        Spi::run(
            "INSERT INTO sparse_events (t, val)
             SELECT '2026-01-01 00:00:00+00'::timestamptz + (i * interval '1 day'), i::double precision
             FROM generate_series(0, 9) i",
        )
        .unwrap();

        let changelog_entries: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'sparse_events'",
        )
        .unwrap()
        .unwrap_or(0);

        // Each row lands in a distinct 1-hour bucket → 10 changelog entries.
        // Before this fix: 1 entry spanning the full 10-day range.
        assert_eq!(
            changelog_entries, 10,
            "sparse insert over 10 days must produce 10 bucketed changelog entries, not 1 amplified span"
        );
    }

    #[pg_test]
    fn test_dense_bulk_insert_deduplicates_same_bucket() {
        // Multiple rows in the same frame bucket must produce a single changelog
        // entry for that bucket — no redundant entries from the GROUP BY.
        Spi::run(
            "CREATE TABLE dense_events (
                t   timestamptz NOT NULL,
                val double precision
            ) WITH (spiral.frames = '1h,1d')",
        )
        .unwrap();

        // 60 rows in the same hour (one per minute) → all in the same 1h bucket.
        Spi::run(
            "INSERT INTO dense_events (t, val)
             SELECT '2026-01-01 00:00:00+00'::timestamptz + (i * interval '1 minute'), i::double precision
             FROM generate_series(0, 59) i",
        )
        .unwrap();

        let changelog_entries: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'dense_events'",
        )
        .unwrap()
        .unwrap_or(0);

        assert_eq!(
            changelog_entries, 1,
            "60 rows in the same 1h bucket must produce exactly 1 changelog entry"
        );
    }

    #[pg_test]
    fn test_sparse_bulk_insert_refresh_only_touches_affected_buckets() {
        // End-to-end: sparse insert over 10 days, refresh, verify rollup has 10
        // hour-buckets populated and changelog is cleared.
        Spi::run(
            "CREATE TABLE sparse_refresh (
                t   timestamptz NOT NULL,
                val double precision -- Spiral: sum
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        Spi::run(
            "INSERT INTO sparse_refresh (t, val)
             SELECT '2026-01-01 00:00:00+00'::timestamptz + (i * interval '1 day'), 1.0
             FROM generate_series(0, 9) i",
        )
        .unwrap();

        Spi::run("SELECT spiral_refresh('sparse_refresh')").unwrap();

        let rollup_rows: i64 =
            Spi::get_one::<i64>("SELECT COUNT(*)::bigint FROM sparse_refresh_1h")
                .unwrap()
                .unwrap_or(0);
        assert_eq!(
            rollup_rows, 10,
            "rollup must have exactly 10 hour-buckets after sparse insert of 10 rows"
        );

        let remaining_changelog: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'sparse_refresh'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            remaining_changelog, 0,
            "changelog must be empty after refresh"
        );
    }

    #[pg_test]
    fn test_status_view_shows_dirty_entries_and_lag() {
        Spi::run(
            "CREATE TABLE obs_events (
                t   timestamptz NOT NULL,
                val double precision
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        // Before any inserts: status row exists, zero dirty entries, NULL lag.
        let dirty_before: i64 = Spi::get_one::<i64>(
            "SELECT dirty_entries FROM spiral.status WHERE base_view = 'obs_events'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            dirty_before, 0,
            "dirty_entries must be 0 before any inserts"
        );

        let lag_before: Option<bool> = Spi::get_one::<bool>(
            "SELECT lag IS NULL FROM spiral.status WHERE base_view = 'obs_events'",
        )
        .unwrap();
        assert_eq!(
            lag_before,
            Some(true),
            "lag must be NULL when rollup is current"
        );

        // Insert rows spanning 3 distinct 1h buckets.
        Spi::run(
            "INSERT INTO obs_events (t, val)
             SELECT '2026-01-01 00:00:00+00'::timestamptz + (i * interval '1 hour'), 1.0
             FROM generate_series(0, 2) i",
        )
        .unwrap();

        let dirty_after: i64 = Spi::get_one::<i64>(
            "SELECT dirty_entries FROM spiral.status WHERE base_view = 'obs_events'",
        )
        .unwrap()
        .unwrap_or(0);
        assert_eq!(dirty_after, 3, "3 distinct hour-buckets → 3 dirty entries");

        let lag_not_null: Option<bool> = Spi::get_one::<bool>(
            "SELECT lag IS NOT NULL FROM spiral.status WHERE base_view = 'obs_events'",
        )
        .unwrap();
        assert_eq!(
            lag_not_null,
            Some(true),
            "lag must be non-NULL when dirty entries exist"
        );

        // After refresh: zero dirty entries, NULL lag again.
        Spi::run("SELECT spiral_refresh('obs_events')").unwrap();

        let dirty_final: i64 = Spi::get_one::<i64>(
            "SELECT dirty_entries FROM spiral.status WHERE base_view = 'obs_events'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(dirty_final, 0, "dirty_entries must be 0 after refresh");
    }

    #[pg_test]
    fn test_spiral_lag_function() {
        Spi::run(
            "CREATE TABLE lag_events (
                t   timestamptz NOT NULL,
                val double precision
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        // No dirty entries → lag is NULL.
        let lag_null: Option<bool> =
            Spi::get_one::<bool>("SELECT spiral_lag('lag_events') IS NULL").unwrap();
        assert_eq!(
            lag_null,
            Some(true),
            "spiral_lag must be NULL when fully current"
        );

        Spi::run("INSERT INTO lag_events (t, val) VALUES (now() - interval '2 hours', 42.0)")
            .unwrap();

        // Dirty entries exist → lag must be a positive interval.
        let lag_positive: Option<bool> =
            Spi::get_one::<bool>("SELECT spiral_lag('lag_events') > interval '0'").unwrap();
        assert_eq!(
            lag_positive,
            Some(true),
            "spiral_lag must be a positive interval when dirty entries exist"
        );

        Spi::run("SELECT spiral_refresh('lag_events')").unwrap();

        let lag_after: Option<bool> =
            Spi::get_one::<bool>("SELECT spiral_lag('lag_events') IS NULL").unwrap();
        assert_eq!(
            lag_after,
            Some(true),
            "spiral_lag must return NULL after refresh clears changelog"
        );
    }

    #[pg_test]
    fn test_status_view_tier_count() {
        Spi::run(
            "CREATE TABLE tiered_obs (
                t   timestamptz NOT NULL,
                val double precision -- Spiral: sum
            ) WITH (spiral.frames = '1h,1d')",
        )
        .unwrap();

        let tier_count: i32 = Spi::get_one::<i32>(
            "SELECT tier_count FROM spiral.status WHERE base_view = 'tiered_obs'",
        )
        .unwrap()
        .unwrap_or(0);
        assert_eq!(tier_count, 2, "frames '1h,1d' must produce tier_count = 2");
    }

    #[pg_test]
    fn test_scope_status_view() {
        Spi::run(
            "CREATE TABLE scope_obs (
                t      timestamptz NOT NULL,
                tenant int         NOT NULL,
                val    double precision
            ) WITH (spiral.frames = '1h', spiral.tenant = 'tenant')",
        )
        .unwrap();

        // No dirty entries → scope_status has no rows for this table.
        let rows_before: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.scope_status WHERE base_view = 'scope_obs'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            rows_before, 0,
            "scope_status must be empty before any inserts"
        );

        // Insert 2 rows for tenant 1 and 1 row for tenant 2 — same time bucket.
        Spi::run(
            "INSERT INTO scope_obs (t, tenant, val) VALUES
             ('2026-01-01 00:00:00+00', 1, 1.0),
             ('2026-01-01 00:00:30+00', 1, 2.0),
             ('2026-01-01 00:00:00+00', 2, 3.0)",
        )
        .unwrap();

        // 2 distinct scopes dirty.
        let scope_count: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.scope_status WHERE base_view = 'scope_obs'",
        )
        .unwrap()
        .unwrap_or(0);
        assert_eq!(scope_count, 2, "2 distinct tenants → 2 scope_status rows");

        // Each scope must report positive lag.
        let all_positive: Option<bool> = Spi::get_one::<bool>(
            "SELECT bool_and(lag > interval '0') FROM spiral.scope_status WHERE base_view = 'scope_obs'",
        )
        .unwrap();
        assert_eq!(
            all_positive,
            Some(true),
            "all dirty scopes must have positive lag"
        );

        // After refresh: scope_status must be empty.
        Spi::run("SELECT spiral_refresh('scope_obs')").unwrap();

        let rows_after: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.scope_status WHERE base_view = 'scope_obs'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(rows_after, 0, "scope_status must be empty after refresh");
    }

    #[pg_test]
    fn test_qa_matrix() {
        let sql = include_str!("../tests/pg_regress/sql/qa_matrix.sql");
        // Strip comments (both standalone and end-of-line)
        let clean_sql = sql
            .lines()
            .map(|line| {
                if let Some(pos) = line.find("--") {
                    &line[..pos]
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Execute statement by statement to isolate failures
        for stmt in clean_sql.split(';') {
            let trimmed = stmt.trim();
            if !trimmed.is_empty() {
                if let Err(e) = Spi::run(trimmed) {
                    panic!(
                        "QA Matrix failed on statement: [{}]. Error: {:?}",
                        trimmed, e
                    );
                }
            }
        }
    }

    #[pg_test]
    fn test_accelerate_idempotency() {
        Spi::run("CREATE TABLE idempotent_test (t timestamptz, val float)").unwrap();

        // First call
        Spi::run("SELECT accelerate('idempotent_test', frames => '1m')").unwrap();

        // Second call with more frames - should not fail due to existing triggers
        Spi::run("SELECT accelerate('idempotent_test', frames => '1m,1h')").unwrap();

        let rollup_exists: bool = Spi::get_one::<bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_class WHERE relname = 'idempotent_test_1h')",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            rollup_exists,
            "Second tier should be created on second accelerate call"
        );
    }

    #[pg_test]
    fn test_subquery_and_join_acceleration() {
        Spi::run("SET timezone = 'UTC'").unwrap();
        Spi::run("CREATE TABLE j1 (t timestamptz, tenant_id int, val float)").unwrap();
        Spi::run("SELECT accelerate('j1', frames => '1h', tenant => ARRAY['tenant_id'])").unwrap();
        Spi::run("INSERT INTO j1 VALUES ('2026-05-25 10:00:00Z', 1, 10)").unwrap();
        Spi::run("INSERT INTO j1 VALUES ('2026-05-25 11:00:00Z', 1, 15)").unwrap(); // Second hour
        Spi::run("SELECT spiral_refresh('j1_1h')").unwrap();

        Spi::run("CREATE TABLE j2 (t timestamptz, tenant_id int, val float)").unwrap();
        Spi::run("SELECT accelerate('j2', frames => '1h', tenant => ARRAY['tenant_id'])").unwrap();
        Spi::run("INSERT INTO j2 VALUES ('2026-05-25 10:00:00Z', 1, 20)").unwrap();
        Spi::run("INSERT INTO j2 VALUES ('2026-05-25 11:00:00Z', 1, 25)").unwrap(); // Second hour
        Spi::run("SELECT spiral_refresh('j2_1h')").unwrap();

        // 1. Subquery acceleration
        // Query for only ONE hour (10:00). If it works, it should show 1 segment of j1_1h.
        let subq_sql = "SELECT * FROM (SELECT sum(val) FROM j1 WHERE t >= ''2026-05-25 10:00:00Z'' AND t < ''2026-05-25 11:00:00Z'') sub";
        let subq_explain =
            Spi::get_one::<String>(&format!("SELECT spiral_explain('{}')", subq_sql))
                .unwrap()
                .unwrap();
        assert!(
            subq_explain.contains("1x j1_1h"),
            "Subquery should use exactly 1 rollup segment"
        );
        assert!(
            subq_explain.contains("Range: 2026-05-25 10:00:00"),
            "Subquery should use the correct start range"
        );

        // 2. JOIN predicate propagation (tenant_id = 1 should propagate from j1 to j2)
        let join_sql = "SELECT * FROM j1 JOIN j2 USING (t, tenant_id) \
                        WHERE j1.t >= ''2026-05-25 10:00:00Z'' AND j1.t < ''2026-05-25 11:00:00Z'' \
                        AND j1.tenant_id = 1";
        let join_explain =
            Spi::get_one::<String>(&format!("SELECT spiral_explain('{}')", join_sql))
                .unwrap()
                .unwrap();

        assert!(
            join_explain.contains("Accelerating 'j1'"),
            "j1 should be accelerated in JOIN"
        );
        assert!(
            join_explain.contains("Accelerating 'j2'"),
            "j2 should be accelerated in JOIN via predicate propagation"
        );

        // Count segments to ensure propagation worked for both
        let j1_accel_line = join_explain
            .lines()
            .find(|l| l.contains("Accelerating 'j1'"))
            .unwrap();
        let j2_accel_line = join_explain
            .lines()
            .find(|l| l.contains("Accelerating 'j2'"))
            .unwrap();

        assert!(
            j1_accel_line.contains("1x j1_1h"),
            "j1 should use 1 segment"
        );
        assert!(
            j2_accel_line.contains("1x j2_1h"),
            "j2 should use 1 segment via propagated time and scope"
        );
    }

    #[pg_test]
    fn test_cost_based_slicing() {
        Spi::run("SET timezone = 'UTC'").unwrap();
        // Create a table with very sparse data (1 record per hour)
        Spi::run("CREATE TABLE sparse (t timestamptz, val float)").unwrap();
        Spi::run("SELECT accelerate('sparse', frames => '1h')").unwrap();
        Spi::run("INSERT INTO sparse VALUES ('2026-05-25 10:30:00Z', 10)").unwrap();
        Spi::run("INSERT INTO sparse VALUES ('2026-05-25 11:30:00Z', 20)").unwrap();
        Spi::run("SELECT spiral_refresh('sparse_1h')").unwrap();

        // Update statistics so pg_class has row counts
        Spi::run("ANALYZE sparse").unwrap();
        Spi::run("ANALYZE sparse_1h").unwrap();

        // In this case, sparse has 2 rows, sparse_1h has 2 rows.
        // The cost model should skip sparse_1h because 2 >= 2 * 0.9.
        let explain = Spi::get_one::<String>("SELECT spiral_explain('SELECT sum(val) FROM sparse WHERE t >= ''2026-05-25 10:00:00Z'' AND t < ''2026-05-25 12:00:00Z''')").unwrap().unwrap();

        // It should NOT contain sparse_1h
        assert!(
            !explain.contains("sparse_1h"),
            "Should not use rollup if it doesn't reduce rows significantly"
        );
        assert!(
            explain.contains("no rollups available"),
            "Should report no rollups available (due to cost model rejection)"
        );
    }

    #[pg_test]
    fn test_changelog_preserved_on_failed_refresh() {
        // If the merge SQL fails (e.g. rollup schema is corrupted), the changelog
        // rows must survive so the planner keeps falling back to raw data and the
        // background worker can retry later.
        Spi::run("CREATE TABLE fail_ticks (t timestamptz NOT NULL, val numeric)").unwrap();
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('fail_ticks', 'BASE', 0, 'fail_ticks', '{}')")
            .unwrap();
        Spi::run(
            "SELECT spiral_register_view('fail_ticks_1h', 'fail_ticks', 3600, 'fail_ticks', '{}')",
        )
        .unwrap();

        Spi::run(
            "INSERT INTO fail_ticks (t, val)
             SELECT now() - interval '2 hours' + (i * interval '20 minutes'), 1.0
             FROM generate_series(0, 5) i",
        )
        .unwrap();

        let before: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'fail_ticks'",
        )
        .unwrap()
        .unwrap_or(0);
        assert!(before > 0, "changelog must have entries before refresh");

        // Break the rollup so MERGE SQL will fail at runtime.
        // A trigger that always raises ensures any INSERT/UPDATE on the rollup aborts.
        Spi::run(
            "CREATE FUNCTION fail_ticks_1h_blocker() RETURNS trigger LANGUAGE plpgsql AS \
             'BEGIN RAISE EXCEPTION ''intentional test failure in rollup''; END'",
        )
        .unwrap();
        Spi::run(
            "CREATE TRIGGER fail_ticks_1h_block BEFORE INSERT OR UPDATE ON fail_ticks_1h \
             FOR EACH ROW EXECUTE FUNCTION fail_ticks_1h_blocker()",
        )
        .unwrap();

        // pgrx converts PostgreSQL ERROR to a Rust panic, which propagates out of
        // spiral_refresh rather than returning Err. Wrap the call in a PL/pgSQL
        // EXCEPTION block so the subtransaction (including refreshing_changelog DDL)
        // rolls back while the outer transaction — and the changelog rows — survive.
        Spi::run(
            "DO $$ BEGIN \
               PERFORM spiral_refresh('fail_ticks'); \
             EXCEPTION WHEN OTHERS THEN \
               NULL; \
             END $$",
        )
        .unwrap();

        let after: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'fail_ticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            after, before,
            "changelog must be unchanged after a failed refresh — got {} rows, expected {}",
            after, before
        );
    }

    #[pg_test]
    fn test_issue_67_repro_delete_leaves_stale_rows() {
        Spi::run("SET spiral.kickoff_date = '2026-04-15';").unwrap();
        Spi::run(
            "CREATE TABLE metrics_repro (
            t timestamptz NOT NULL,
            device_id text NOT NULL,
            val double precision -- Spiral: sum
        ) WITH (
            spiral.frames = '1m',
            spiral.tenant = 'device_id'
        );",
        )
        .unwrap();

        // 1. Ingest initial data
        Spi::run(
            "INSERT INTO metrics_repro (t, device_id, val) VALUES
            ('2026-04-15 10:00:05Z', 'A', 10.0),
            ('2026-04-15 10:00:55Z', 'A', 20.0);",
        )
        .unwrap();

        // 2. Refresh
        Spi::run("SELECT spiral_refresh('metrics_repro');").unwrap();

        // Check metrics_repro_1m (should have 1 row for A at 10:00:00)
        let count: i64 = Spi::get_one("SELECT count(*) FROM metrics_repro_1m")
            .unwrap()
            .unwrap();
        assert_eq!(count, 1, "Initial refresh should produce 1 row");

        // 3. Delete ALL data for that bucket
        Spi::run("DELETE FROM metrics_repro WHERE device_id = 'A';").unwrap();

        // 4. Refresh again
        Spi::run("SELECT spiral_refresh('metrics_repro');").unwrap();

        // 5. Check metrics_repro_1m (it SHOULD BE EMPTY)
        let count: i64 = Spi::get_one("SELECT count(*) FROM metrics_repro_1m")
            .unwrap()
            .unwrap();
        assert_eq!(
            count, 0,
            "Refresh after delete should result in 0 rows in rollup"
        );
    }

    // issue #66: scoped rollups must have a unique index on (t, scope_cols)
    #[pg_test]
    fn test_scoped_rollup_unique_index() {
        Spi::run(
            "CREATE TABLE scoped_u (t timestamptz NOT NULL, tenant int NOT NULL, val double precision)",
        )
        .unwrap();
        Spi::run("SELECT accelerate('scoped_u', frames => '1h', tenant => ARRAY['tenant'])")
            .unwrap();

        let has_unique: bool = Spi::get_one::<bool>(
            "SELECT EXISTS (
                SELECT 1 FROM pg_indexes
                WHERE tablename = 'scoped_u_1h'
                  AND indexdef LIKE '%UNIQUE%'
                  AND indexdef LIKE '%tenant%'
            )",
        )
        .unwrap()
        .unwrap_or(false);
        assert!(
            has_unique,
            "scoped rollup must have UNIQUE index on (t, tenant)"
        );
    }

    // issue #68: dirty range [t, t+bucket) must not mark the next frame dirty
    #[pg_test]
    fn test_dirty_range_no_overexpansion() {
        Spi::run("SET timezone = 'UTC'").unwrap();
        Spi::run("CREATE TABLE boundary_ticks (t timestamptz NOT NULL, val float)").unwrap();
        Spi::run("SELECT accelerate('boundary_ticks', frames => '1h')").unwrap();

        // Insert into last minute of hour 0 (frame [0h, 1h))
        Spi::run("INSERT INTO boundary_ticks VALUES ('2026-01-01 00:59:00+00', 1.0)").unwrap();

        // Refresh — only frame [0h, 1h) should be touched
        Spi::run("SELECT spiral_refresh('boundary_ticks')").unwrap();

        // After refresh, changelog must be empty (no stale dirty entries for hour 1+)
        let dirty: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM spiral.changelog WHERE base_view = 'boundary_ticks'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(
            dirty, 0,
            "changelog must be empty after refresh — no over-expanded dirty range"
        );

        // Insert into first minute of hour 1 (frame [1h, 2h))
        Spi::run("INSERT INTO boundary_ticks VALUES ('2026-01-01 01:00:00+00', 2.0)").unwrap();

        Spi::run("SELECT spiral_refresh('boundary_ticks')").unwrap();

        // Both hours should have exactly 1 rollup row each
        let hour0: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM boundary_ticks_1h
             WHERE t = '2026-01-01 00:00:00+00'",
        )
        .unwrap()
        .unwrap_or(0);
        let hour1: i64 = Spi::get_one::<i64>(
            "SELECT COUNT(*)::bigint FROM boundary_ticks_1h
             WHERE t = '2026-01-01 01:00:00+00'",
        )
        .unwrap()
        .unwrap_or(0);
        assert_eq!(hour0, 1, "hour 0 rollup row must exist");
        assert_eq!(hour1, 1, "hour 1 rollup row must exist");
    }

    #[pg_test]
    fn test_count_star_without_where_clause() {
        // Regression: count(*) on a Spiral-accelerated table with rolled-up data
        // must return the correct total row count without "unboxing other_ argument
        // failed" — caused by a stale aggtranstype (INT8) on the rewritten Aggref
        // making PostgreSQL store JSONB transition states as byval pointers, which
        // became dangling when the COMBINEFUNC merged partial results from UNION ALL
        // arms whose memory contexts had been freed.
        Spi::run(
            "CREATE TABLE count_test (
                t   timestamptz NOT NULL,
                val double precision  -- Spiral: sum
            ) WITH (spiral.frames = '1h')",
        )
        .unwrap();

        Spi::run(
            "INSERT INTO count_test (t, val)
             SELECT '2026-01-01 00:00:00+00'::timestamptz + (i * interval '10 minutes'), i::float8
             FROM generate_series(0, 11) i",
        )
        .unwrap();

        Spi::run("SELECT spiral_refresh('count_test')").unwrap();

        // count(*) with WHERE clause — must not error
        let n_with_where: i64 = Spi::get_one(
            "SELECT count(*) FROM count_test
             WHERE t >= '2026-01-01 00:00:00+00' AND t < '2026-01-01 02:00:00+00'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(n_with_where, 12, "count(*) with WHERE must return 12");

        // count(*) without WHERE clause — was broken before the aggtranstype fix
        let n_no_where: i64 = Spi::get_one("SELECT count(*) FROM count_test")
            .unwrap()
            .unwrap_or(-1);
        assert_eq!(n_no_where, 12, "count(*) without WHERE must return 12");

        // DROP TABLE must clean up spiral catalog entries
        Spi::run("DROP TABLE count_test CASCADE").unwrap();
        let meta_gone: i64 = Spi::get_one(
            "SELECT COUNT(*)::bigint FROM spiral.metadata WHERE view_name = 'count_test'",
        )
        .unwrap()
        .unwrap_or(-1);
        assert_eq!(meta_gone, 0, "spiral.metadata must be cleaned up after DROP TABLE");
    }
}

#[cfg(test)]
mod zorder_tests {
    use crate::zorder::*;

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
        // t is no longer masked to 32 bits.
        let t_over_u32 = u32::MAX as i64 + 1; // 2^32
        assert_ne!(
            spiral_zorder_int_array_core(t_over_u32, vec![0]),
            spiral_zorder_int_array_core(0, vec![0])
        );
    }

    #[test]
    fn test_zorder_no_collision_within_full_range() {
        // Distinct values must produce distinct results (no collision).
        let r1 = spiral_zorder_int_array_core(1 << 16, vec![0]);
        let r2 = spiral_zorder_int_array_core((1 << 16) + 1, vec![0]);
        assert_ne!(r1, r2);

        // And bit 32 of t is preserved.
        let r3 = spiral_zorder_int_array_core(1_i64 << 32, vec![0]);
        assert_ne!(r3, spiral_zorder_int_array_core(0, vec![0]));
    }

    #[test]
    fn test_zorder_ordering_preserved() {
        // Increasing t with same dimension must yield strictly increasing z-order.
        let results: Vec<u128> = (0..4)
            .map(|t| spiral_zorder_int_array_core(t, vec![0]))
            .collect();
        for w in results.windows(2) {
            assert!(w[0] < w[1], "z-order not monotone: {} >= {}", w[0], w[1]);
        }
    }

    #[test]
    fn test_zorder_u32_boundary() {
        // Values around u32::MAX encode correctly and in order.
        let near_max = u32::MAX as i64 - 1;
        let at_max = u32::MAX as i64;
        let r1 = spiral_zorder_int_array_core(near_max, vec![0]);
        let r2 = spiral_zorder_int_array_core(at_max, vec![0]);
        assert!(
            r1 < r2,
            "z-order not monotone near u32::MAX: {} >= {}",
            r1,
            r2
        );
    }

    #[test]
    fn test_zorder_3d_uses_42_bits() {
        // Bit 41 of x (the 42nd bit) maps to output position 3*41=123.
        let x = 1i64 << 41;
        let result = zorder_3d_core(x, 0, 0);
        let expected: u128 = 1u128 << 123;
        assert_eq!(result, expected);
    }

    #[test]
    fn test_zorder_deterministic() {
        let r1 = spiral_zorder_core(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        let r2 = spiral_zorder_core(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        assert_eq!(r1, r2);
    }

    #[test]
    fn test_zorder_string_hash_stable() {
        // Pin FNV-1a outputs.
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"sensor_a"), 11_461_482_958_241_160_951_u64);

        // Distinct strings must hash to different values.
        assert_ne!(fnv1a_64(b"sensor_a"), fnv1a_64(b"sensor_b"));

        // z-order output must be self-consistent across calls.
        let r1 = spiral_zorder_core(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        let r2 = spiral_zorder_core(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        assert_eq!(r1, r2, "z-order must be deterministic across calls");

        // Different tenant strings must yield different z-order values (for same t).
        let za = spiral_zorder_core(0, vec![Some("tenant_a".to_string())]);
        let zb = spiral_zorder_core(0, vec![Some("tenant_b".to_string())]);
        assert_ne!(za, zb, "different tenant strings must not collide");
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
            assert!(
                w[0] < w[1],
                "z-order not monotone for fixed tenant: {} >= {} (t values: {:?})",
                w[0],
                w[1],
                timestamps
            );
        }
    }
}
