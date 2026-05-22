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

pgrx::pg_module_magic!();

extension_sql_file!("../sql/spiral.sql", name = "spiral_setup");

pub static WORKER_ENABLED: GucSetting<bool> = GucSetting::<bool>::new(true);
pub static WORKER_DEBUG: GucSetting<bool> = GucSetting::<bool>::new(false);
pub static WORKER_MAX: GucSetting<i32> = GucSetting::<i32>::new(1);

thread_local! {
    pub static SKIP_ACCELERATION: Cell<bool> = const { Cell::new(false) };
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
}

/// Computes the Z-order curve for a timestamp and a set of string dimensions.
///
/// Hashes string dimensions and interleaves their bits with the time bit representation.
///
/// Time is encoded using the low 32 bits of `t` (Unix epoch seconds), giving coverage
/// from 1970-01-01 to approximately 2106-02-07 without loss of ordering information.
///
/// # Examples
/// ```rust
/// use spiral::spiral_zorder;
///
/// let res = spiral_zorder(0, vec![]);
/// assert_eq!(res, 0);
/// ```
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder(t: i64, dimensions: Vec<Option<String>>) -> i64 {
    // Mask t to 32 bits so the bit-spreading step (which maps input bit k → output bit 2k)
    // cannot have bits 32+ collide with correctly-placed bits from bits 0–31.
    // This gives correct ordering for t in [0, 2^32) ≈ Unix epoch seconds until ~2106.
    let mut x = (t as u64) & 0x0000_0000_FFFF_FFFF;
    let mut y = 0u64;
    for (i, dim) in dimensions.iter().enumerate() {
        if let Some(d) = dim {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            use std::hash::Hasher;
            hasher.write(d.as_bytes());
            let hash = hasher.finish();
            y ^= hash << (i % 8);
        }
    }
    y &= 0x0000_0000_FFFF_FFFF;

    // Spread 32 bits of each input into the even/odd bit positions of a 64-bit result.
    x = (x | (x << 16)) & 0x0000FFFF0000FFFF_u64;
    x = (x | (x << 8)) & 0x00FF00FF00FF00FF_u64;
    x = (x | (x << 4)) & 0x0F0F0F0F0F0F0F0F_u64;
    x = (x | (x << 2)) & 0x3333333333333333_u64;
    x = (x | (x << 1)) & 0x5555555555555555_u64;

    y = (y | (y << 16)) & 0x0000FFFF0000FFFF_u64;
    y = (y | (y << 8)) & 0x00FF00FF00FF00FF_u64;
    y = (y | (y << 4)) & 0x0F0F0F0F0F0F0F0F_u64;
    y = (y | (y << 2)) & 0x3333333333333333_u64;
    y = (y | (y << 1)) & 0x5555555555555555_u64;

    (x | (y << 1)) as i64
}

/// Computes the Z-order curve for a timestamp and a set of integer dimensions.
///
/// Interleaves the lower bits of the dimensions with the time bit representation.
///
/// Time is encoded using the low 32 bits of `t` (Unix epoch seconds), giving correct
/// ordering for t in [0, 2^32) — Unix epoch coverage from 1970-01-01 to ~2106-02-07.
///
/// # Examples
/// ```rust
/// use spiral::spiral_zorder_int_array;
///
/// let res = spiral_zorder_int_array(1, vec![1]);
/// assert_eq!(res, 3);
/// ```
#[pg_extern(immutable, parallel_safe, name = "spiral_zorder")]
pub fn spiral_zorder_int_array(t: i64, dimensions: Vec<i32>) -> i64 {
    // Mask t to 32 bits so the bit-spreading step (which maps input bit k → output bit 2k)
    // cannot have bits 32+ collide with correctly-placed bits from bits 0–31.
    let mut x = (t as u64) & 0x0000_0000_FFFF_FFFF;
    let mut y = 0u64;
    for (i, dim) in dimensions.iter().enumerate() {
        y ^= (*dim as u64) << (i % 8);
    }
    y &= 0x0000_0000_FFFF_FFFF;

    // Spread 32 bits of each input into the even/odd bit positions of a 64-bit result.
    x = (x | (x << 16)) & 0x0000FFFF0000FFFF_u64;
    x = (x | (x << 8)) & 0x00FF00FF00FF00FF_u64;
    x = (x | (x << 4)) & 0x0F0F0F0F0F0F0F0F_u64;
    x = (x | (x << 2)) & 0x3333333333333333_u64;
    x = (x | (x << 1)) & 0x5555555555555555_u64;

    y = (y | (y << 16)) & 0x0000FFFF0000FFFF_u64;
    y = (y | (y << 8)) & 0x00FF00FF00FF00FF_u64;
    y = (y | (y << 4)) & 0x0F0F0F0F0F0F0F0F_u64;
    y = (y | (y << 2)) & 0x3333333333333333_u64;
    y = (y | (y << 1)) & 0x5555555555555555_u64;

    (x | (y << 1)) as i64
}

/// Computes the 3D Z-order curve for a timestamp (`x`) and two integer dimensions (`y` and `z`).
///
/// Interleaves 21 bits from each of the three inputs into a 63-bit result (63 = 21 × 3),
/// fitting within the positive range of i64. This gives `x` a time-domain range of 2^21
/// seconds (~24 days) per period; callers should normalize `x` to a suitable epoch if needed.
///
/// # Examples
/// ```rust
/// use spiral::spiral_zorder_3d;
///
/// let res = spiral_zorder_3d(1, 1, 1);
/// assert_eq!(res, 7);
///
/// let res2 = spiral_zorder_3d(2, 0, 0);
/// assert_eq!(res2, 8);
/// ```
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_zorder_3d(x: i64, y: i32, z: i32) -> i64 {
    let mut res = 0i64;
    for i in 0..21 {
        res |= ((x >> i) & 1) << (3 * i);
        res |= (((y as i64) >> i) & 1) << (3 * i + 1);
        res |= (((z as i64) >> i) & 1) << (3 * i + 2);
    }
    res
}

/// Computes the 2D bit interleaving (similar to Hilbert curve pre-processing) for two integer dimensions.
///
/// # Examples
/// ```rust
/// use spiral::spiral_hilbert_2d;
///
/// let res = spiral_hilbert_2d(1, 1);
/// assert_eq!(res, 3);
/// ```
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_hilbert_2d(x: i32, y: i32) -> i32 {
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

const POSTGRES_EPOCH_JDATE: i64 = 946684800; // seconds between 1970-01-01 and 2000-01-01

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
                .map(|s| format!("'{}', \"{}\"", s, s))
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
    }

    let children = catalog::get_children(view_name);
    for child in children {
        if child != view_name {
            let _ = refresh_incremental(&child, extra_where.clone(), depth + 1);
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
    let (sql, sources) =
        rollup::derive_child_sql(view_name, source_table, frame_seconds, &scope_columns);

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
            let index_sql = if scope_columns.is_empty() {
                format!(
                    "CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{view_name} ON {view_name}(t)"
                )
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
    fn test_planner_rejects_grouped_sum_target_lists_for_now() {
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
            assert!(cols.is_none());
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

            let cols = hooks::extract_supported_query_columns(
                query,
                (*query).rtable,
                "planner_datetrunc",
            );
            assert!(
                cols.is_some(),
                "date_trunc in target list should not block acceleration"
            );
            let cols = cols.unwrap();
            assert!(
                cols.iter().any(|(name, agg)| name == "val" && agg.as_deref() == Some("sum")),
                "sum(val) should be in cols: {:?}",
                cols
            );
        }
    }

    #[pg_test]
    fn test_planner_rejects_unsupported_aggregate_target_lists() {
        Spi::run(
            "CREATE TABLE planner_fallback (t timestamptz, tenant_id int, val double precision)",
        )
        .unwrap();

        for sql in [
            "SELECT avg(val) FROM planner_fallback",
            "SELECT count(*) FROM planner_fallback",
            "SELECT min(val), max(val) FROM planner_fallback",
            "SELECT sum(val), avg(val) FROM planner_fallback",
            "SELECT sum(val + 1) FROM planner_fallback",
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
        use crate::spiral_zorder_int_array;

        // Test case 1: 0, 0 -> 0
        assert_eq!(spiral_zorder_int_array(0, vec![0]), 0);

        // Test case 2: 1, 0 -> 1 (Time bit 0 at position 0)
        assert_eq!(spiral_zorder_int_array(1, vec![0]), 1);

        // Test case 3: 0, 1 -> 2 (Dimension bit 0 at position 1)
        assert_eq!(spiral_zorder_int_array(0, vec![1]), 2);

        // Test case 4: 1, 1 -> 3 (Both bits 0 at positions 0 and 1)
        assert_eq!(spiral_zorder_int_array(1, vec![1]), 3);

        // Test case 5: 2, 0 -> 4 (Time bit 1 at position 2)
        assert_eq!(spiral_zorder_int_array(2, vec![0]), 4);

        // Test case 6: 0, 2 -> 8 (Dimension bit 1 at position 3)
        assert_eq!(spiral_zorder_int_array(0, vec![2]), 8);
    }

    #[pg_test]
    fn test_hilbert_2d_correctness() {
        use crate::spiral_hilbert_2d;
        assert_eq!(spiral_hilbert_2d(0, 0), 0);
        assert_eq!(spiral_hilbert_2d(1, 0), 1);
        assert_eq!(spiral_hilbert_2d(0, 1), 2);
        assert_eq!(spiral_hilbert_2d(1, 1), 3);
    }
}

#[cfg(test)]
mod zorder_tests {
    use super::*;

    #[test]
    fn test_zorder_zero() {
        assert_eq!(spiral_zorder(0, vec![]), 0);
    }

    #[test]
    fn test_zorder_int_array_basic() {
        assert_eq!(spiral_zorder_int_array(1, vec![1]), 3);
    }

    #[test]
    fn test_zorder_3d_basic() {
        assert_eq!(spiral_zorder_3d(1, 1, 1), 7);
        assert_eq!(spiral_zorder_3d(2, 0, 0), 8);
    }

    #[test]
    fn test_zorder_large_timestamp_wraps_at_2_32() {
        // t is masked to 32 bits before spreading; values ≥ 2^32 cycle back.
        // t=0 and t=2^32 both have low 32 bits = 0, so they produce the same result.
        let t_over_u32 = u32::MAX as i64 + 1; // 2^32
        assert_eq!(
            spiral_zorder_int_array(t_over_u32, vec![0]),
            spiral_zorder_int_array(0, vec![0])
        );
    }

    #[test]
    fn test_zorder_no_collision_within_u32_range() {
        // Distinct values in [0, 2^32) must produce distinct results (no collision).
        let r1 = spiral_zorder_int_array(1 << 16, vec![0]);
        let r2 = spiral_zorder_int_array((1 << 16) + 1, vec![0]);
        assert_ne!(r1, r2);
        // And bit 32 of t does NOT interfere with bit 16 (was a bug without explicit masking).
        let r3 = spiral_zorder_int_array(1_i64 << 32, vec![0]); // wraps to 0
        assert_eq!(r3, spiral_zorder_int_array(0, vec![0]));
    }

    #[test]
    fn test_zorder_ordering_preserved() {
        // Increasing t with same dimension must yield strictly increasing z-order.
        let results: Vec<i64> = (0..4)
            .map(|t| spiral_zorder_int_array(t, vec![0]))
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
        let r1 = spiral_zorder_int_array(near_max, vec![0]);
        let r2 = spiral_zorder_int_array(at_max, vec![0]);
        assert!(
            r1 < r2,
            "z-order not monotone near u32::MAX: {} >= {}",
            r1,
            r2
        );
    }

    #[test]
    fn test_zorder_3d_uses_21_bits() {
        // Bit 20 of x (index 20, the 21st bit) maps to output position 3*20=60.
        let x = 1i64 << 20;
        let result = spiral_zorder_3d(x, 0, 0);
        assert_eq!(result, 1i64 << 60);
    }

    #[test]
    fn test_zorder_deterministic() {
        let r1 = spiral_zorder(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        let r2 = spiral_zorder(
            12345,
            vec![Some("sensor_a".to_string()), Some("region_eu".to_string())],
        );
        assert_eq!(r1, r2);
    }
}
