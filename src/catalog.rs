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
}

/// Invalidate all per-session catalog caches. Call after any DDL that touches
/// spiral.metadata or the set of rollup views.
pub fn invalidate_catalog_cache() {
    METADATA_TABLE_EXISTS.with(|c| c.set(None));
    HIERARCHY_CACHE.with(|c| c.borrow_mut().clear());
    METADATA_CACHE.with(|c| c.borrow_mut().clear());
    OFFSET_COLS_CACHE.with(|c| c.borrow_mut().clear());
}

fn spiral_metadata_table_exists() -> bool {
    if let Some(cached) = METADATA_TABLE_EXISTS.with(|c| c.get()) {
        return cached;
    }
    let exists = Spi::connect(|client| {
        Ok::<bool, spi::Error>(
            !client.select(
                "SELECT 1 FROM information_schema.tables \
                 WHERE table_schema = 'spiral' AND table_name = 'metadata' LIMIT 1",
                Some(1),
                &[],
            )?.is_empty()
        )
    }).unwrap_or(false);
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
            &format!("SELECT view_name FROM spiral.metadata WHERE base_view = '{}'",
                     base_table.replace("'", "''")),
            None, &[])?;
        for row in table {
            v.push(row.get::<String>(1)?.unwrap_or_default());
        }
        Ok::<Vec<String>, spi::Error>(v)
    }).unwrap_or_default();
    HIERARCHY_CACHE.with(|c| c.borrow_mut().insert(base_table.to_string(), views.clone()));
    views
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

pub fn unify_changelog(base_view: &str) {
    // Snapshot existing rows with their ctids first. Any concurrent inserts that
    // arrive after this point will have new ctids and won't be touched by the
    // subsequent DELETE, preserving them for the next refresh cycle.
    let _ = Spi::run(&format!(
        "CREATE TEMP TABLE changelog_snapshot AS SELECT ctid AS old_ctid, * FROM spiral.changelog WHERE base_view = '{}'",
        base_view.replace("'", "''")
    ));
    let _ = Spi::run("CREATE TEMP TABLE temp_unified AS
         SELECT base_view, scope_values, MIN(t_start) as ts, MAX(t_end) as te
         FROM (
            SELECT *,
                COUNT(*) FILTER (WHERE prev_end < t_start OR prev_end IS NULL) OVER (PARTITION BY base_view, scope_values ORDER BY t_start) as grp
            FROM (
                SELECT *,
                    MAX(t_end) OVER (PARTITION BY base_view, scope_values ORDER BY t_start ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) as prev_end
                FROM changelog_snapshot
            ) s1
         ) s2
         GROUP BY base_view, scope_values, grp");
    // Only delete the rows we snapshotted; concurrent inserts survive.
    let _ = Spi::run(
        "DELETE FROM spiral.changelog WHERE ctid IN (SELECT old_ctid FROM changelog_snapshot)",
    );
    let _ = Spi::run("INSERT INTO spiral.changelog (base_view, scope_values, t_start, t_end) SELECT base_view, scope_values, ts, te FROM temp_unified");
    let _ = Spi::run("DROP TABLE changelog_snapshot");
    let _ = Spi::run("DROP TABLE temp_unified");
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
                           AND NOT (t_end < {} OR t_start > {})
                           AND (scope_values = '{{}}'::jsonb OR scope_values = '{}'::jsonb)
                         ORDER BY t_start",
                base_view.replace("'", "''"),
                ts,
                te,
                sv_json.replace("'", "''")
            )
        } else {
            format!(
                "SELECT t_start, t_end FROM spiral.changelog
                         WHERE base_view = '{}'
                           AND NOT (t_end < {} OR t_start > {})
                         ORDER BY t_start",
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
        if let Some(serde_json::Value::String(s)) = map.get("cardinality") {
            return match s.as_str() {
                "d" => 10,
                "h" => 100,
                "k" => 1000,
                "M" => 1000000,
                "B" => 1000000000,
                "T" => 1000000000000,
                _ => 1024,
            };
        }
    }
    1024
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
