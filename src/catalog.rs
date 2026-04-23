use pgrx::prelude::*;
use pgrx::datum::DatumWithOid;

extension_sql!(
    r#"
    CREATE SCHEMA IF NOT EXISTS aspiral;
    
    CREATE TABLE IF NOT EXISTS aspiral.metadata (
        view_name text PRIMARY KEY,
        parent_view text NOT NULL,
        frame_seconds integer NOT NULL,
        base_view text NOT NULL,
        scope_columns text[] NOT NULL DEFAULT '{}',
        columns_metadata jsonb NOT NULL DEFAULT '{}',
        created_at timestamptz DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS aspiral.changelog (
        base_view text NOT NULL,
        t_start bigint NOT NULL,
        t_end bigint NOT NULL,
        scope_values jsonb NOT NULL DEFAULT '{}'
    );
    CREATE INDEX IF NOT EXISTS idx_aspiral_changelog_base ON aspiral.changelog (base_view);

    CREATE TABLE IF NOT EXISTS aspiral.sources (
        id serial PRIMARY KEY,
        view_name text NOT NULL,
        base_view text NOT NULL,
        frame_seconds integer NOT NULL,
        base_column text NOT NULL,
        formula text NOT NULL,
        mat_column text NOT NULL,
        args jsonb NOT NULL DEFAULT '{}'
    );
    CREATE INDEX IF NOT EXISTS idx_aspiral_sources_lookup ON aspiral.sources(base_view, base_column, formula);
    
    CREATE OR REPLACE FUNCTION aspiral.track_changes_stmt() RETURNS trigger AS $$
    BEGIN
        IF (TG_OP = 'INSERT' OR TG_OP = 'UPDATE') THEN
            RAISE NOTICE 'Aspiral Trigger: Tracking INSERT/UPDATE on %', TG_ARGV[0];
            INSERT INTO aspiral.changelog (base_view, t_start, t_end, scope_values)
            SELECT TG_ARGV[0], MIN(aspiral(t)), MAX(aspiral(t)), '{}'::jsonb FROM new_table;
        END IF;
        IF (TG_OP = 'DELETE' OR TG_OP = 'UPDATE') THEN
            RAISE NOTICE 'Aspiral Trigger: Tracking DELETE/UPDATE on %', TG_ARGV[0];
            INSERT INTO aspiral.changelog (base_view, t_start, t_end, scope_values)
            SELECT TG_ARGV[0], MIN(aspiral(t)), MAX(aspiral(t)), '{}'::jsonb FROM old_table;
        END IF;
        RETURN NULL;
    END;
    $$ LANGUAGE plpgsql;

    -- Hybrid view to show both manual sources and potentially rules/views
    CREATE OR REPLACE VIEW aspiral.available_sources AS
    SELECT view_name, base_view, frame_seconds, base_column, formula, mat_column, args
    FROM aspiral.sources;
    "#,
    name = "create_aspiral_metadata"
);

pub fn insert_source(view_name: &str, base_view: &str, frame_seconds: i32, base_column: &str, formula: &str, mat_column: &str, args: pgrx::JsonB) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.sources (view_name, base_view, frame_seconds, base_column, formula, mat_column, args) 
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[
                DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(frame_seconds.into_datum(), PgBuiltInOids::INT4OID.value()),
                DatumWithOid::new(base_column.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(formula.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(mat_column.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(args.into_datum(), PgBuiltInOids::JSONBOID.value()),
            ],
        ).unwrap();
    }
}

pub fn mark_range_dirty(base_view: &str, t_start: i64, t_end: i64, scope_values: pgrx::JsonB) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.changelog (base_view, t_start, t_end, scope_values) VALUES ($1, $2, $3, $4)",
            &[
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(t_start.into_datum(), PgBuiltInOids::INT8OID.value()),
                DatumWithOid::new(t_end.into_datum(), PgBuiltInOids::INT8OID.value()),
                DatumWithOid::new(scope_values.into_datum(), PgBuiltInOids::JSONBOID.value()),
            ],
        ).unwrap();
    }
}

pub fn unify_changelog(base_view: &str) {
    // This function implements the "joining unions of segments" logic.
    // It merges overlapping or adjacent segments for the same base_view and scope_values.
    let _ = Spi::connect(|_client| {
         let _ = Spi::run(&format!(
            "CREATE TEMP TABLE temp_unified AS 
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

#[allow(dead_code)]
pub fn get_dirty_buckets(base_view: &str) -> Vec<(i64, pgrx::JsonB)> {
    Spi::connect(|client| {
        let mut res = Vec::new();
        // Generate series of minute buckets (60s) for each segment
        let tuple_table = client.select(
            "SELECT DISTINCT b, scope_values 
             FROM aspiral.changelog, 
                  generate_series(t_start, t_end, 60) as b 
             WHERE base_view = $1",
            None,
            &[Some(base_view.into_datum()).into()]
        )?;
        for row in tuple_table {
            let t = row.get::<i64>(1).unwrap().unwrap();
            let sv = row.get::<pgrx::JsonB>(2).unwrap().unwrap();
            res.push((t, sv));
        }
        Ok::<Vec<(i64, pgrx::JsonB)>, spi::Error>(res)
    }).unwrap_or_default()
}

#[allow(dead_code)]
pub fn clear_dirty_buckets(base_view: &str, buckets: &[(i64, pgrx::JsonB)]) {
    if buckets.is_empty() { return; }
    for (t, sv) in buckets {
        unsafe {
            Spi::run_with_args(
                "DELETE FROM aspiral.changelog WHERE base_view = $1 AND bucket_t = $2 AND scope_values = $3",
                &[
                    DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                    DatumWithOid::new(t.into_datum(), PgBuiltInOids::INT8OID.value()),
                    DatumWithOid::new(pgrx::JsonB(sv.0.clone()).into_datum(), PgBuiltInOids::JSONBOID.value()),
                ]
            ).unwrap();
        }
    }
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

pub struct Metadata {
    pub parent_view: String,
    #[allow(dead_code)]
    pub frame_seconds: i32,
    pub scope_columns: Vec<String>,
    pub columns_metadata: pgrx::JsonB,
}

pub fn get_metadata(view_name: &str) -> Option<Metadata> {
    Spi::connect(|client| {
        let row = unsafe {
            client.select(
                "SELECT parent_view, frame_seconds, scope_columns, columns_metadata FROM aspiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap().first()
        };
        if row.is_empty() { return Ok::<Option<Metadata>, spi::Error>(None); }
        Ok(Some(Metadata {
            parent_view: row.get::<String>(1).unwrap().unwrap(),
            frame_seconds: row.get::<i32>(2).unwrap().unwrap(),
            scope_columns: row.get::<Vec<String>>(3).unwrap().unwrap(),
            columns_metadata: row.get::<pgrx::JsonB>(4).unwrap().unwrap(),
        }))
    }).unwrap_or(None)
}

pub fn get_children(view_name: &str) -> Vec<String> {
    Spi::connect(|client| {
        let mut children = Vec::new();
        let tuple_table = client.select(
            "SELECT view_name FROM aspiral.metadata WHERE parent_view = $1 ORDER BY frame_seconds ASC",
            None,
            unsafe { &[pgrx::datum::DatumWithOid::new(view_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        )?;
        for row in tuple_table {
            let child = row.get::<String>(1).unwrap().unwrap();
            children.push(child);
        }
        Ok::<Vec<String>, spi::Error>(children)
    }).unwrap_or_default()
}

pub fn get_dirty_ranges(base_view: &str, ts: i64, te: i64) -> Vec<(i64, i64)> {
    Spi::connect(|client| {
        let mut ranges = Vec::new();
        let tuple_table = client.select(
            "SELECT t_start, t_end FROM aspiral.changelog WHERE base_view = $1 AND NOT (t_end < $2 OR t_start > $3) ORDER BY t_start",
            None,
            unsafe { &[
                pgrx::datum::DatumWithOid::new(base_view.into_datum().unwrap(), pg_sys::TEXTOID),
                pgrx::datum::DatumWithOid::new(ts.into_datum().unwrap(), pg_sys::INT8OID),
                pgrx::datum::DatumWithOid::new(te.into_datum().unwrap(), pg_sys::INT8OID),
            ] }
        )?;
        for row in tuple_table {
            let s = row.get::<i64>(1).unwrap().unwrap();
            let e = row.get::<i64>(2).unwrap().unwrap();
            ranges.push((s, e));
        }
        Ok::<Vec<(i64, i64)>, spi::Error>(ranges)
    }).unwrap_or_default()
}
