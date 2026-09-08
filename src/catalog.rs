use pgrx::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

#[derive(Clone)]
pub struct Metadata {
    pub parent_view: String,
    pub frame_seconds: i32,
    pub base_view: String,
    pub scope_columns: Vec<String>,
    pub columns_metadata: serde_json::Value,
}

#[derive(Clone, Debug)]
pub struct TimelineEpoch {
    pub start_t: i64,
    pub end_t: i64,
    pub tenant_scale: i64,
    pub base_offset: i64,
}

thread_local! {
    /// Cached result of checking whether spiral.metadata exists.
    /// None = unchecked, Some(false) = absent, Some(true) = present.
    static METADATA_TABLE_EXISTS: Cell<Option<bool>> = const { Cell::new(None) };
    /// base_table → ordered list of rollup view names (the hierarchy).
    static HIERARCHY_CACHE: RefCell<HashMap<String, Vec<String>>> = RefCell::new(HashMap::new());
    /// view_name → metadata row (None = confirmed absent).
    static METADATA_CACHE: RefCell<HashMap<String, Option<Metadata>>> = RefCell::new(HashMap::new());
    /// view_name → offset columns.
    static OFFSET_COLS_CACHE: RefCell<HashMap<String, Vec<OffsetColumn>>> = RefCell::new(HashMap::new());
    /// table_name -> timeline epochs
    static TIMELINE_CACHE: RefCell<HashMap<String, Vec<TimelineEpoch>>> = RefCell::new(HashMap::new());
}

/// Invalidate all per-session catalog caches. Call after any DDL that touches
/// spiral.metadata or the set of rollup views.
pub fn invalidate_catalog_cache() {
    METADATA_TABLE_EXISTS.with(|c| c.set(None));
    HIERARCHY_CACHE.with(|c| c.borrow_mut().clear());
    METADATA_CACHE.with(|c| c.borrow_mut().clear());
    OFFSET_COLS_CACHE.with(|c| c.borrow_mut().clear());
    TIMELINE_CACHE.with(|c| c.borrow_mut().clear());
}

pub fn get_timeline(table_name: &str) -> Vec<TimelineEpoch> {
    let cached = TIMELINE_CACHE.with(|c| c.borrow().get(table_name).cloned());
    if let Some(v) = cached {
        return v;
    }

    let epochs = Spi::connect(|client| {
        let sql = format!(
            "SELECT start_t, COALESCE(end_t, 9223372036854775807), tenant_scale, base_offset 
             FROM spiral.tenants_timeline 
             WHERE table_name = '{}' 
             ORDER BY start_t ASC",
            table_name.replace("'", "''")
        );
        let mut results = Vec::new();
        match client.select(&sql, None, &[]) {
            Ok(tuple_table) => {
                for row in tuple_table {
                    results.push(TimelineEpoch {
                        start_t: row.get::<i64>(1)?.unwrap_or(0),
                        end_t: row.get::<i64>(2)?.unwrap_or(i64::MAX),
                        tenant_scale: row.get::<i32>(3)?.unwrap_or(1024) as i64,
                        base_offset: row.get::<i64>(4)?.unwrap_or(0),
                    });
                }
            }
            Err(_) => {
                // Return silently on error, usually happens when mid-transaction in a hook
            }
        }
        Ok::<Vec<TimelineEpoch>, spi::Error>(results)
    })
    .unwrap_or_default();

    TIMELINE_CACHE.with(|c| {
        c.borrow_mut()
            .insert(table_name.to_string(), epochs.clone())
    });
    epochs
}

pub fn compute_slot_index(
    t_rel: i64,
    tenant_id: i64,
    epochs: &[TimelineEpoch],
    fallback_scale: i64,
) -> i64 {
    if epochs.is_empty() {
        return (t_rel * fallback_scale) + tenant_id;
    }
    for epoch in epochs {
        if t_rel >= epoch.start_t && t_rel < epoch.end_t {
            let offset_in_epoch = t_rel - epoch.start_t;
            return epoch.base_offset + (offset_in_epoch * epoch.tenant_scale) + tenant_id;
        }
    }
    let first = &epochs[0];
    if t_rel < first.start_t {
        let offset = t_rel - first.start_t;
        return first.base_offset + (offset * first.tenant_scale) + tenant_id;
    }
    let last = epochs.last().unwrap();
    let offset = t_rel - last.start_t;
    last.base_offset + (offset * last.tenant_scale) + tenant_id
}

pub fn reverse_slot_index(
    slot_index: i64,
    epochs: &[TimelineEpoch],
    fallback_scale: i64,
) -> (i64, i64) {
    if epochs.is_empty() {
        return (slot_index / fallback_scale, slot_index % fallback_scale);
    }

    // Since base_offset grows, we can find the epoch by finding the last one where slot_index >= base_offset
    let mut target_epoch = &epochs[0];
    for epoch in epochs.iter().rev() {
        if slot_index >= epoch.base_offset {
            target_epoch = epoch;
            break;
        }
    }

    let offset_in_epoch = slot_index - target_epoch.base_offset;
    let t_rel = target_epoch.start_t + (offset_in_epoch / target_epoch.tenant_scale);
    let tid = offset_in_epoch % target_epoch.tenant_scale;

    (t_rel, tid)
}

pub fn compute_tenant_scale(t_rel: i64, epochs: &[TimelineEpoch], fallback_scale: i64) -> i64 {
    if epochs.is_empty() {
        return fallback_scale;
    }
    for epoch in epochs {
        if t_rel >= epoch.start_t && t_rel < epoch.end_t {
            return epoch.tenant_scale;
        }
    }
    epochs.last().unwrap().tenant_scale
}

fn spiral_metadata_table_exists() -> bool {
    if let Some(cached) = METADATA_TABLE_EXISTS.with(|c| c.get()) {
        return cached;
    }
    let exists = Spi::connect(|client| {
        Ok::<bool, spi::Error>(
            !client
                .select(
                    "SELECT 1 FROM information_schema.tables \
                 WHERE table_schema = 'spiral' AND table_name = 'metadata' LIMIT 1",
                    Some(1),
                    &[],
                )?
                .is_empty(),
        )
    })
    .unwrap_or(false);
    METADATA_TABLE_EXISTS.with(|c| c.set(Some(exists)));
    exists
}

/// Returns the ordered list of rollup view names for `base_table`, cached per session.
pub fn get_hierarchy(base_table: &str) -> Vec<String> {
    let cached = HIERARCHY_CACHE.with(|c| c.borrow().get(base_table).cloned());
    if let Some(v) = cached {
        return v;
    }
    if !spiral_metadata_table_exists() {
        return vec![];
    }
    let views = Spi::connect(|client| {
        let mut v = Vec::new();
        let table = client.select(
            &format!(
                "SELECT view_name FROM spiral.metadata WHERE base_view = '{}'",
                base_table.replace("'", "''")
            ),
            None,
            &[],
        )?;
        for row in table {
            v.push(row.get::<String>(1)?.unwrap_or_default());
        }
        Ok::<Vec<String>, spi::Error>(v)
    })
    .unwrap_or_default();
    HIERARCHY_CACHE.with(|c| c.borrow_mut().insert(base_table.to_string(), views.clone()));
    views
}

pub fn get_table_stats(relname: &str) -> (f64, i32) {
    let (t, p) = Spi::get_two::<f64, i32>(&format!(
        "SELECT reltuples::float8, relpages FROM pg_class WHERE oid = to_regclass('\"{}\"')",
        relname.replace("\"", "\"\"")
    ))
    .unwrap_or_default();
    let t = t.unwrap_or(0.0);
    (if t < 0.0 { 0.0 } else { t }, p.unwrap_or(0))
}

pub fn get_metadata(view_name: &str) -> Option<Metadata> {
    let cached = METADATA_CACHE.with(|c| c.borrow().get(view_name).cloned());
    if let Some(entry) = cached {
        return entry;
    }

    let result = Spi::connect(|client| {
        let table = client.select(
            &format!("SELECT parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata WHERE view_name = '{}'", view_name.replace("'", "''")),
            None,
            &[]
        )?;
        if table.is_empty() {
            return Ok::<Option<Metadata>, spi::Error>(None);
        }
        let row = table.first();
        Ok(Some(Metadata {
            parent_view: row.get::<String>(1)?.unwrap_or_default(),
            frame_seconds: row.get::<i32>(2)?.unwrap_or(0),
            base_view: row.get::<String>(3)?.unwrap_or_default(),
            scope_columns: row.get::<Vec<String>>(4)?.unwrap_or_default(),
            columns_metadata: row.get::<pgrx::JsonB>(5)?.map(|j| j.0).unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
        }))
    }).unwrap_or_default();
    METADATA_CACHE.with(|c| c.borrow_mut().insert(view_name.to_string(), result.clone()));
    result
}

pub fn get_children(view_name: &str) -> Vec<String> {
    Spi::connect(|client| {
        let mut children = Vec::new();
        let tuple_table = client.select(
            &format!("SELECT view_name FROM spiral.metadata WHERE parent_view = '{}' ORDER BY frame_seconds ASC", view_name.replace("'", "''")),
            None,
            &[]
        )?;
        for row in tuple_table {
            if let Ok(Some(child)) = row.get::<String>(1) {
                children.push(child);
            }
        }
        Ok::<Vec<String>, spi::Error>(children)
    }).unwrap_or_default()
}

pub fn is_spiral_relation(name: &str) -> bool {
    Spi::connect(|client| {
        let table = client.select(
            &format!(
                "SELECT 1 FROM spiral.metadata WHERE view_name = '{}'",
                name.replace("'", "''")
            ),
            None,
            &[],
        )?;
        Ok::<bool, spi::Error>(!table.is_empty())
    })
    .unwrap_or(false)
}

pub fn insert_metadata(
    view_name: &str,
    parent_view: &str,
    frame_seconds: i32,
    base_view: &str,
    scope_columns: Vec<String>,
    columns_metadata: pgrx::JsonB,
) {
    let scope_cols_json =
        serde_json::to_string(&scope_columns).unwrap_or_else(|_| "[]".to_string());
    let metadata_json =
        serde_json::to_string(&columns_metadata.0).unwrap_or_else(|_| "{}".to_string());

    let sql = format!(
        "INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata)
         VALUES ('{}', '{}', {}, '{}', '{}'::text[], '{}'::jsonb)
         ON CONFLICT (view_name) DO UPDATE SET parent_view = EXCLUDED.parent_view, frame_seconds = EXCLUDED.frame_seconds, base_view = EXCLUDED.base_view, scope_columns = EXCLUDED.scope_columns, columns_metadata = EXCLUDED.columns_metadata",
        view_name.replace("'", "''"),
        parent_view.replace("'", "''"),
        frame_seconds,
        base_view.replace("'", "''"),
        scope_cols_json.replace("[", "{").replace("]", "}"), // Simple array format
        metadata_json.replace("'", "''")
    );
    let _ = Spi::run(&sql);
    // Inserted new metadata — invalidate so the next planner lookup picks it up.
    invalidate_catalog_cache();
}

#[allow(clippy::too_many_arguments)]
pub fn insert_source(
    view_name: &str,
    base_view: &str,
    frame_seconds: i32,
    base_column: &str,
    formula: &str,
    mat_column: &str,
    rollup_gsub_strategy: Option<&str>,
    metadata: pgrx::JsonB,
) {
    let rgs_val = if let Some(s) = rollup_gsub_strategy {
        format!("'{}'", s.replace("'", "''"))
    } else {
        "NULL".to_string()
    };
    let metadata_json = serde_json::to_string(&metadata.0).unwrap_or_else(|_| "{}".to_string());

    let sql = format!(
        "INSERT INTO spiral.sources (view_name, base_view, frame_seconds, base_column, formula, mat_column, rollup_gsub_strategy, metadata)
         VALUES ('{0}', '{1}', {2}, '{3}', '{4}', '{5}', {6}, '{7}'::jsonb)
         ON CONFLICT (view_name, base_column, formula) DO UPDATE SET base_view = EXCLUDED.base_view, frame_seconds = EXCLUDED.frame_seconds, mat_column = EXCLUDED.mat_column, rollup_gsub_strategy = EXCLUDED.rollup_gsub_strategy, metadata = EXCLUDED.metadata",
        view_name.replace("'", "''"),
        base_view.replace("'", "''"),
        frame_seconds,
        base_column.replace("'", "''"),
        formula.replace("'", "''"),
        mat_column.replace("'", "''"),
        rgs_val,
        metadata_json.replace("'", "''")
    );

    let _ = Spi::run(&sql);
}

pub fn unify_changelog_scope(base_view: &str, scope_json: &str) {
    let safe_bv = base_view.replace("'", "''");
    let safe_sv = scope_json.replace("'", "''");
    let _ = Spi::run(&format!(
        "CREATE TEMP TABLE scope_cl_snapshot AS \
         SELECT ctid AS old_ctid, * FROM spiral.changelog \
         WHERE base_view = '{}' AND scope_values = '{}'::jsonb",
        safe_bv, safe_sv
    ));
    let _ = Spi::run("CREATE TEMP TABLE scope_cl_unified AS
         SELECT base_view, scope_values,
                NULLIF(MIN(ts_safe), -9223372036854775808) as ts,
                NULLIF(MAX(te_safe), 9223372036854775807) as te
         FROM (
            SELECT *,
                COUNT(*) FILTER (WHERE prev_end < ts_safe OR prev_end IS NULL) OVER (PARTITION BY base_view, scope_values ORDER BY ts_safe) as grp
            FROM (
                SELECT *,
                    COALESCE(t_start, -9223372036854775808) as ts_safe,
                    COALESCE(t_end, 9223372036854775807) as te_safe,
                    MAX(COALESCE(t_end, 9223372036854775807)) OVER (PARTITION BY base_view, scope_values ORDER BY COALESCE(t_start, -9223372036854775808) ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) as prev_end
                FROM scope_cl_snapshot
            ) s1
         ) s2
         GROUP BY base_view, scope_values, grp");
    let _ = Spi::run(
        "DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM scope_cl_snapshot)",
    );
    let _ = Spi::run("INSERT INTO spiral.changelog (base_view, scope_values, t_start, t_end) SELECT base_view, scope_values, ts, te FROM scope_cl_unified");
    let _ = Spi::run("DROP TABLE scope_cl_snapshot");
    let _ = Spi::run("DROP TABLE scope_cl_unified");
}

pub fn unify_changelog(base_view: &str) {
    // Snapshot existing rows with their ctids first.
    Spi::run(&format!(
        "CREATE TEMP TABLE changelog_snapshot AS SELECT ctid AS old_ctid, * FROM spiral.changelog WHERE base_view = '{}'",
        base_view.replace("'", "''")
    )).unwrap();

    // Unify logic using sentinels for NULLs (unbounded ranges)
    Spi::run("CREATE TEMP TABLE temp_unified AS
         SELECT base_view, scope_values, 
                NULLIF(MIN(ts_safe), -9223372036854775808) as ts,
                NULLIF(MAX(te_safe), 9223372036854775807) as te
         FROM (
            SELECT *,
                COUNT(*) FILTER (WHERE prev_end < ts_safe OR prev_end IS NULL) OVER (PARTITION BY base_view, scope_values ORDER BY ts_safe) as grp
            FROM (
                SELECT *,
                    COALESCE(t_start, -9223372036854775808) as ts_safe,
                    COALESCE(t_end, 9223372036854775807) as te_safe,
                    MAX(COALESCE(t_end, 9223372036854775807)) OVER (PARTITION BY base_view, scope_values ORDER BY COALESCE(t_start, -9223372036854775808) ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) as prev_end
                FROM changelog_snapshot
            ) s1
         ) s2
         GROUP BY base_view, scope_values, grp").unwrap();

    Spi::run(
        "DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM changelog_snapshot)",
    )
    .unwrap();
    Spi::run("INSERT INTO spiral.changelog (base_view, scope_values, t_start, t_end) SELECT base_view, scope_values, ts, te FROM temp_unified").unwrap();
    Spi::run("DROP TABLE changelog_snapshot").unwrap();
    Spi::run("DROP TABLE temp_unified").unwrap();
}

/// Groups dirty changelog scopes sharing an identical (t_start, t_end) range
/// into batches capped at `max_scopes_per_batch`, so a single wide bulk-load
/// range touching thousands of scopes doesn't produce either a giant
/// `scope_values IN (...)` clause or a one-scope-at-a-time refresh loop.
/// Call after `unify_changelog` so ranges are already merged per scope.
pub fn coalesce_changelog_batches(base_view: &str, max_scopes_per_batch: i64) -> Vec<Vec<String>> {
    let safe_bv = base_view.replace('\'', "''");
    Spi::connect(|client| {
        let sql = format!(
            "SELECT array_agg(scope_values::text) FROM (
                SELECT scope_values,
                       (row_number() OVER (PARTITION BY t_start, t_end ORDER BY scope_values) - 1)
                         / {} AS batch_id
                FROM spiral.changelog WHERE base_view = '{}'
             ) s GROUP BY t_start, t_end, batch_id",
            max_scopes_per_batch, safe_bv
        );
        Ok::<Vec<Vec<String>>, spi::Error>(
            client
                .select(&sql, None, &[])?
                .map(|r| r.get::<Vec<String>>(1).unwrap().unwrap_or_default())
                .collect(),
        )
    })
    .unwrap_or_default()
}

pub fn get_dirty_ranges(
    base_view: &str,
    ts: i64,
    te: i64,
    scope_values: Option<pgrx::JsonB>,
) -> Vec<(i64, i64)> {
    Spi::connect(|client| {
        let mut ranges = Vec::new();
        let sql = if let Some(sv) = scope_values {
            let sv_json = serde_json::to_string(&sv.0).unwrap_or_else(|_| "{}".to_string());
            format!(
                "SELECT t_start, t_end FROM spiral.changelog
                         WHERE base_view = '{}'
                           AND (t_end IS NULL OR t_end > {})
                           AND (t_start IS NULL OR t_start < {})
                           AND (scope_values = '{{}}'::jsonb OR scope_values = '{}'::jsonb)
                         ORDER BY t_start NULLS FIRST",
                base_view.replace("'", "''"),
                ts,
                te,
                sv_json.replace("'", "''")
            )
        } else {
            format!(
                "SELECT t_start, t_end FROM spiral.changelog
                         WHERE base_view = '{}'
                           AND (t_end IS NULL OR t_end > {})
                           AND (t_start IS NULL OR t_start < {})
                         ORDER BY t_start NULLS FIRST",
                base_view.replace("'", "''"),
                ts,
                te
            )
        };

        let tuple_table = client.select(&sql, None, &[])?;
        for row in tuple_table {
            let s = row.get::<i64>(1)?.unwrap_or(0);
            let e = row.get::<i64>(2)?.unwrap_or(0);
            ranges.push((s, e));
        }
        Ok::<Vec<(i64, i64)>, spi::Error>(ranges)
    })
    .unwrap_or_default()
}

pub fn get_tenant_scale(metadata: &Metadata) -> i64 {
    if let serde_json::Value::Object(map) = &metadata.columns_metadata {
        if let Some(val) = map.get("tenant_scale") {
            if let Some(n) = val.as_i64() {
                return if n > 0 {
                    (n as u64).next_power_of_two() as i64
                } else {
                    1
                };
            }
        }
        if let Some(serde_json::Value::String(s)) = map.get("cardinality") {
            return match s.as_str() {
                "d" => 16,
                "h" => 128,
                "k" => 1024,
                "M" => 1048576,
                "B" => 1073741824,
                "T" => 1099511627776,
                _ => 1024,
            };
        }
    }
    1024
}

pub fn get_kickoff(metadata: &Metadata) -> i64 {
    if let serde_json::Value::Object(map) = &metadata.columns_metadata {
        if let Some(val) = map.get("kickoff_epoch") {
            if let Some(n) = val.as_i64() {
                return n;
            }
        }
    }
    crate::get_kickoff_epoch()
}

#[derive(Clone)]
pub struct OffsetColumn {
    pub mat_column: String,
    pub formula: String,
}

pub fn get_offset_columns(view_name: &str) -> Vec<OffsetColumn> {
    let cached = OFFSET_COLS_CACHE.with(|c| c.borrow().get(view_name).cloned());
    if let Some(v) = cached {
        return v;
    }
    let cols = Spi::connect(|client| {
        let sql = format!(
            "SELECT mat_column, formula FROM spiral.sources
             WHERE view_name = '{}' AND formula IN ('range_max_end', 'range_merge')",
            view_name.replace("'", "''")
        );
        Ok::<Vec<OffsetColumn>, spi::Error>(
            client
                .select(&sql, None, &[])?
                .map(|r| OffsetColumn {
                    mat_column: r.get::<String>(1).unwrap().unwrap(),
                    formula: r.get::<String>(2).unwrap().unwrap(),
                })
                .collect(),
        )
    })
    .unwrap_or_default();
    OFFSET_COLS_CACHE.with(|c| c.borrow_mut().insert(view_name.to_string(), cols.clone()));
    cols
}

/// Remove all Spiral catalog entries for a dropped table.
/// Cleans spiral.metadata, spiral.sources, and spiral.changelog rows whose
/// base_view or view_name match `table_name` (handles both base tables and rollup tiers).
/// Cancels any Spiral Worker query touching this table so locks are released immediately.
/// If no Spiral tables remain after cleanup, cancels all Spiral Workers for this DB.
pub fn remove_table_from_spiral(table_name: &str) {
    if !spiral_metadata_table_exists() {
        return;
    }
    let name = table_name.replace("'", "''");

    // Cancel any Spiral Worker currently processing this table so its transaction
    // aborts and releases table locks before we delete the catalog rows.
    // Cancel any Spiral Worker currently processing this table so its transaction
    // aborts immediately before we touch the catalog or drop hierarchy tables.
    let _ = Spi::run(&format!(
        "SELECT pg_cancel_backend(pid) \
         FROM pg_stat_activity \
         WHERE backend_type LIKE 'Spiral Worker%' \
           AND query LIKE '%{name}%'"
    ));

    // Collect hierarchy table names BEFORE deleting them from the catalog so we
    // can DROP the actual PG tables. The hierarchy tables have no PG dependency
    // on the base table, so DROP TABLE base CASCADE won't reach them.
    let hierarchy_tables: Vec<String> = Spi::connect(|client| {
        Ok::<Vec<String>, spi::Error>(
            client
                .select(
                    &format!(
                        "SELECT view_name FROM spiral.metadata \
                     WHERE base_view = '{name}' AND view_name <> '{name}'"
                    ),
                    None,
                    &[],
                )?
                .map(|row| row.get::<String>(1).unwrap_or(None).unwrap_or_default())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>(),
        )
    })
    .unwrap_or_default();

    // Drop hierarchy tables (they outlive the base table without this step).
    for ht in &hierarchy_tables {
        let safe_ht = ht.replace('\'', "''");
        let _ = Spi::run(&format!("DROP TABLE IF EXISTS \"{safe_ht}\" CASCADE"));
    }

    let _ = Spi::run(&format!(
        "DELETE FROM spiral.sources   WHERE view_name = '{name}' OR base_view = '{name}'; \
         DELETE FROM spiral.metadata  WHERE view_name = '{name}' OR base_view = '{name}'; \
         DELETE FROM spiral.changelog WHERE base_view = '{name}';"
    ));

    // If no base tables remain, cancel all Spiral Workers — nothing left to process.
    let remaining =
        Spi::get_one::<i64>("SELECT COUNT(*) FROM spiral.metadata WHERE parent_view = 'BASE'")
            .ok()
            .flatten()
            .unwrap_or(1);

    if remaining == 0 {
        let _ = Spi::run(
            "SELECT pg_cancel_backend(pid) \
             FROM pg_stat_activity \
             WHERE backend_type LIKE 'Spiral Worker%'",
        );
    }

    invalidate_catalog_cache();
}
