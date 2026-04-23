use pgrx::prelude::*;
use pgrx::pg_sys;
use pgrx::datum::DatumWithOid;

pub struct Metadata {
    pub parent_view: String,
    pub frame_seconds: i32,
    pub base_view: String,
    pub scope_columns: Vec<String>,
    pub columns_metadata: pgrx::JsonB,
}

pub fn get_metadata(view_name: &str) -> Option<Metadata> {
    Spi::connect(|client| {
        let row = unsafe {
            client.select(
                "SELECT parent_view, frame_seconds, base_view, scope_columns, columns_metadata FROM aspiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(view_name.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap().first()
        };
        if row.is_empty() { 
            return Ok::<Option<Metadata>, spi::Error>(None); 
        }
        Ok(Some(Metadata {
            parent_view: row.get::<String>(1).unwrap().unwrap(),
            frame_seconds: row.get::<i32>(2).unwrap().unwrap(),
            base_view: row.get::<String>(3).unwrap().unwrap(),
            scope_columns: row.get::<Vec<String>>(4).unwrap().unwrap(),
            columns_metadata: row.get::<pgrx::JsonB>(5).unwrap().unwrap(),
        }))
    }).unwrap_or_else(|e| {
        notice!("Aspiral: get_metadata error for '{}': {:?}", view_name, e);
        None
    })
}

pub fn get_children(view_name: &str) -> Vec<String> {
    Spi::connect(|client| {
        let mut children = Vec::new();
        let tuple_table = unsafe {
            client.select(
                "SELECT view_name FROM aspiral.metadata WHERE parent_view = $1 ORDER BY frame_seconds ASC",
                None,
                &[DatumWithOid::new(view_name.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap()
        };
        for row in tuple_table {
            let child = row.get::<String>(1).unwrap().unwrap();
            children.push(child);
        }
        Ok::<Vec<String>, spi::Error>(children)
    }).unwrap_or_else(|e| {
        notice!("Aspiral: get_children error for '{}': {:?}", view_name, e);
        vec![]
    })
}

pub fn is_aspiral_relation(name: &str) -> bool {
    Spi::connect(|client| {
        let row = unsafe {
            client.select(
                "SELECT 1 FROM aspiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(name.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap().first()
        };
        Ok::<bool, spi::Error>(!row.is_empty())
    }).unwrap_or(false)
}

pub fn insert_metadata(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str, scope_columns: Vec<String>, columns_metadata: pgrx::JsonB) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns, columns_metadata) 
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (view_name) DO UPDATE SET parent_view = EXCLUDED.parent_view, frame_seconds = EXCLUDED.frame_seconds, base_view = EXCLUDED.base_view, scope_columns = EXCLUDED.scope_columns, columns_metadata = EXCLUDED.columns_metadata",
            &[
                DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(parent_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(frame_seconds.into_datum(), PgBuiltInOids::INT4OID.value()),
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(scope_columns.into_datum(), PgBuiltInOids::TEXTARRAYOID.value()),
                DatumWithOid::new(columns_metadata.into_datum(), PgBuiltInOids::JSONBOID.value()),
            ],
        ).unwrap();
    }
}

pub fn insert_source(view_name: &str, base_view: &str, frame_seconds: i32, base_column: &str, formula: &str, mat_column: &str, metadata: pgrx::JsonB) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.sources (view_name, base_view, frame_seconds, base_column, formula, mat_column, metadata) 
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (view_name, base_column, formula) DO UPDATE SET base_view = EXCLUDED.base_view, frame_seconds = EXCLUDED.frame_seconds, mat_column = EXCLUDED.mat_column, metadata = EXCLUDED.metadata",
            &[
                DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(frame_seconds.into_datum(), PgBuiltInOids::INT4OID.value()),
                DatumWithOid::new(base_column.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(formula.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(mat_column.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(metadata.into_datum(), PgBuiltInOids::JSONBOID.value()),
            ],
        ).unwrap();
    }
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
                    FROM aspiral.changelog
                    WHERE base_view = '{}'
                ) s1
             ) s2
             GROUP BY base_view, scope_values, grp", base_view.replace("'", "''")));

         let _ = Spi::run(&format!("DELETE FROM aspiral.changelog WHERE base_view = '{}'", base_view.replace("'", "''")));
         let _ = Spi::run(&format!("INSERT INTO aspiral.changelog (base_view, scope_values, t_start, t_end) SELECT base_view, scope_values, ts, te FROM temp_unified"));
         let _ = Spi::run("DROP TABLE temp_unified");
         Ok::<(), spi::Error>(())
    });
}

pub fn get_dirty_ranges(base_view: &str, ts: i64, te: i64) -> Vec<(i64, i64)> {
    Spi::connect(|client| {
        let mut ranges = Vec::new();
        let tuple_table = unsafe {
            client.select(
                "SELECT t_start, t_end FROM aspiral.changelog WHERE base_view = $1 AND NOT (t_end < $2 OR t_start > $3) ORDER BY t_start",
                None,
                &[
                    DatumWithOid::new(base_view.into_datum().unwrap(), PgBuiltInOids::TEXTOID.value()),
                    DatumWithOid::new(ts.into_datum().unwrap(), PgBuiltInOids::INT8OID.value()),
                    DatumWithOid::new(te.into_datum().unwrap(), PgBuiltInOids::INT8OID.value()),
                ]
            ).unwrap()
        };
        for row in tuple_table {
            let s = row.get::<i64>(1).unwrap().unwrap();
            let e = row.get::<i64>(2).unwrap().unwrap();
            ranges.push((s, e));
        }
        Ok::<Vec<(i64, i64)>, spi::Error>(ranges)
    }).unwrap_or_default()
}
