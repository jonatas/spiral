use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;

pub struct Metadata {
    pub parent_view: String,
    pub frame_seconds: i32,
    pub base_view: String,
    pub scope_columns: Vec<String>,
    pub columns_metadata: pgrx::JsonB,
}

pub fn get_metadata(view_name: &str) -> Option<Metadata> {
    Spi::connect(|client| {
        let table = unsafe {
            client.select(
                "SELECT parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM spiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(view_name.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value())]
            )?
        };
        if table.is_empty() {
            return Ok::<Option<Metadata>, spi::Error>(None);
        }
        let row = table.first();
        Ok(Some(Metadata {
            parent_view: row.get::<String>(1)?.unwrap_or_default(),
            frame_seconds: row.get::<i32>(2)?.unwrap_or(0),
            base_view: row.get::<String>(3)?.unwrap_or_default(),
            scope_columns: row.get::<Vec<String>>(4)?.unwrap_or_default(),
            columns_metadata: row.get::<pgrx::JsonB>(5)?.unwrap_or_else(|| pgrx::JsonB(serde_json::Value::Object(serde_json::Map::new()))),
        }))
    }).unwrap_or_else(|e| {
        notice!("Spiral: get_metadata error for '{}': {:?}", view_name, e);
        None
    })
}

pub fn get_children(view_name: &str) -> Vec<String> {
    Spi::connect(|client| {
        let mut children = Vec::new();
        let tuple_table = unsafe {
            client.select(
                "SELECT view_name FROM spiral.metadata WHERE parent_view = $1 ORDER BY frame_seconds ASC",
                None,
                &[DatumWithOid::new(view_name.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value())]
            )?
        };
        for row in tuple_table {
            if let Ok(Some(child)) = row.get::<String>(1) {
                children.push(child);
            }
        }
        Ok::<Vec<String>, spi::Error>(children)
    }).unwrap_or_else(|e| {
        notice!("Spiral: get_children error for '{}': {:?}", view_name, e);
        vec![]
    })
}

pub fn is_spiral_relation(name: &str) -> bool {
    Spi::connect(|client| {
        let table = unsafe {
            client.select(
                "SELECT 1 FROM spiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(
                    name.into_datum(),
                    PgBuiltInOids::TEXTOID.value(),
                )],
            )?
        };
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
    let _ = Spi::connect(|_client| unsafe {
        Spi::run_with_args(
                "INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (view_name) DO UPDATE SET parent_view = EXCLUDED.parent_view, frame_seconds = EXCLUDED.frame_seconds, base_view = EXCLUDED.base_view, scope_columns = EXCLUDED.scope_columns, columns_metadata = EXCLUDED.columns_metadata",
                &[
                    DatumWithOid::new(view_name.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value()),
                    DatumWithOid::new(parent_view.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value()),
                    DatumWithOid::new(frame_seconds.into_datum().unwrap(), PgBuiltInOids::INT4OID.value()),
                    DatumWithOid::new(base_view.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value()),
                    DatumWithOid::new(scope_columns.into_datum().unwrap(), PgBuiltInOids::TEXTARRAYOID.value()),
                    DatumWithOid::new(columns_metadata.into_datum().unwrap(), PgBuiltInOids::JSONBOID.value()),
                ],
            )
    });
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

    let sql = format!(
        "INSERT INTO spiral.sources (view_name, base_view, frame_seconds, base_column, formula, mat_column, rollup_gsub_strategy, metadata)
         VALUES ($1, $2, $3, $4, $5, $6, {}, $7)
         ON CONFLICT (view_name, base_column, formula) DO UPDATE SET base_view = EXCLUDED.base_view, frame_seconds = EXCLUDED.frame_seconds, mat_column = EXCLUDED.mat_column, rollup_gsub_strategy = EXCLUDED.rollup_gsub_strategy, metadata = EXCLUDED.metadata",
        rgs_val
    );

    let _ = Spi::connect(|_client| unsafe {
        Spi::run_with_args(
            &sql,
            &[
                DatumWithOid::new(
                    view_name.into_datum().unwrap(),
                    PgBuiltInOids::TEXTOID.value(),
                ),
                DatumWithOid::new(
                    base_view.into_datum().unwrap(),
                    PgBuiltInOids::TEXTOID.value(),
                ),
                DatumWithOid::new(
                    frame_seconds.into_datum().unwrap(),
                    PgBuiltInOids::INT4OID.value(),
                ),
                DatumWithOid::new(
                    base_column.into_datum().unwrap(),
                    PgBuiltInOids::TEXTOID.value(),
                ),
                DatumWithOid::new(
                    formula.into_datum().unwrap(),
                    PgBuiltInOids::TEXTOID.value(),
                ),
                DatumWithOid::new(
                    mat_column.into_datum().unwrap(),
                    PgBuiltInOids::TEXTOID.value(),
                ),
                DatumWithOid::new(
                    metadata.into_datum().unwrap(),
                    PgBuiltInOids::JSONBOID.value(),
                ),
            ],
        )
    });
}

pub fn unify_changelog(base_view: &str) {
    let _ = Spi::connect(|_client| {
        let _ = Spi::run(&format!("CREATE TEMP TABLE temp_unified AS
             SELECT base_view, scope_values, MIN(t_start) as ts, MAX(t_end) as te
             FROM (
                SELECT *,
                    COUNT(*) FILTER (WHERE prev_end < t_start OR prev_end IS NULL) OVER (PARTITION BY base_view, scope_values ORDER BY t_start) as grp
                FROM (
                    SELECT *,
                        MAX(t_end) OVER (PARTITION BY base_view, scope_values ORDER BY t_start ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING) as prev_end
                    FROM spiral.changelog
                    WHERE base_view = '{}'
                ) s1
             ) s2
             GROUP BY base_view, scope_values, grp", base_view.replace("'", "''")));

        let _ = Spi::run(&format!(
            "DELETE FROM spiral.changelog WHERE base_view = '{}'",
            base_view.replace("'", "''")
        ));
        let _ = Spi::run("INSERT INTO spiral.changelog (base_view, scope_values, t_start, t_end) SELECT base_view, scope_values, ts, te FROM temp_unified");
        let _ = Spi::run("DROP TABLE temp_unified");
        Ok::<(), spi::Error>(())
    });
}

pub fn get_dirty_ranges(
    base_view: &str,
    ts: i64,
    te: i64,
    scope_values: Option<pgrx::JsonB>,
) -> Vec<(i64, i64)> {
    Spi::connect(|client| {
        let mut ranges = Vec::new();
        let sql = if scope_values.is_some() {
            "SELECT t_start, t_end FROM spiral.changelog
                     WHERE base_view = $1
                       AND NOT (t_end < $2 OR t_start > $3)
                       AND (scope_values = '{}'::jsonb OR scope_values = $4)
                     ORDER BY t_start"
                .to_string()
        } else {
            "SELECT t_start, t_end FROM spiral.changelog
                     WHERE base_view = $1
                       AND NOT (t_end < $2 OR t_start > $3)
                     ORDER BY t_start"
                .to_string()
        };

        let mut args = Vec::new();
        unsafe {
            args.push(DatumWithOid::new(
                base_view.into_datum().unwrap(),
                PgBuiltInOids::TEXTOID.value(),
            ));
            args.push(DatumWithOid::new(
                ts.into_datum().unwrap(),
                PgBuiltInOids::INT8OID.value(),
            ));
            args.push(DatumWithOid::new(
                te.into_datum().unwrap(),
                PgBuiltInOids::INT8OID.value(),
            ));
            if let Some(sv) = scope_values {
                args.push(DatumWithOid::new(
                    sv.into_datum(),
                    PgBuiltInOids::JSONBOID.value(),
                ));
            }
        }

        let tuple_table = client.select(&sql, None, &args).unwrap();
        for row in tuple_table {
            let s = row.get::<i64>(1).unwrap().unwrap();
            let e = row.get::<i64>(2).unwrap().unwrap();
            ranges.push((s, e));
        }
        Ok::<Vec<(i64, i64)>, spi::Error>(ranges)
    })
    .unwrap_or_default()
}

pub fn get_tenant_scale(metadata: &Metadata) -> i64 {
    if let serde_json::Value::Object(map) = &metadata.columns_metadata.0 {
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
