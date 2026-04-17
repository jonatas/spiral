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
        created_at timestamptz DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS aspiral.changelog (
        base_view text NOT NULL,
        bucket_t bigint NOT NULL,
        scope_values jsonb NOT NULL DEFAULT '{}',
        PRIMARY KEY (base_view, bucket_t, scope_values)
    );
    "#,
    name = "create_aspiral_metadata"
);

pub fn mark_bucket_dirty(base_view: &str, t: i64, scope_values: pgrx::JsonB) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.changelog (base_view, bucket_t, scope_values) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            &[
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(t.into_datum(), PgBuiltInOids::INT8OID.value()),
                DatumWithOid::new(scope_values.into_datum(), PgBuiltInOids::JSONBOID.value()),
            ],
        ).unwrap();
    }
}

pub fn get_dirty_buckets(base_view: &str) -> Vec<(i64, pgrx::JsonB)> {
    Spi::connect(|client| {
        let mut res = Vec::new();
        let tuple_table = unsafe {
            client.select(
                "SELECT bucket_t, scope_values FROM aspiral.changelog WHERE base_view = $1",
                None,
                &[DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap()
        };
        for row in tuple_table {
            let t = row.get::<i64>(1).unwrap().unwrap();
            let sv = row.get::<pgrx::JsonB>(2).unwrap().unwrap();
            res.push((t, sv));
        }
        Ok::<Vec<(i64, pgrx::JsonB)>, spi::Error>(res)
    }).unwrap_or_default()
}

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

pub fn insert_metadata(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str, scope_columns: Vec<String>) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) 
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (view_name) DO UPDATE SET parent_view = EXCLUDED.parent_view, frame_seconds = EXCLUDED.frame_seconds, base_view = EXCLUDED.base_view, scope_columns = EXCLUDED.scope_columns",
            &[
                DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(parent_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(frame_seconds.into_datum(), PgBuiltInOids::INT4OID.value()),
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(scope_columns.into_datum(), PgBuiltInOids::TEXTARRAYOID.value()),
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

pub fn get_metadata(view_name: &str) -> Option<(Vec<String>, i32)> {
    Spi::connect(|client| {
        let row = unsafe {
            client.select(
                "SELECT scope_columns, frame_seconds FROM aspiral.metadata WHERE view_name = $1",
                None,
                &[DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap().first()
        };
        if row.is_empty() { return Ok::<Option<(Vec<String>, i32)>, spi::Error>(None); }
        let cols = row.get::<Vec<String>>(1).unwrap().unwrap();
        let frame = row.get::<i32>(2).unwrap().unwrap();
        Ok(Some((cols, frame)))
    }).unwrap_or(None)
}

pub fn get_children(view_name: &str) -> Vec<String> {
    Spi::connect(|client| {
        let mut children = Vec::new();
        let tuple_table = unsafe {
            // pgrx 0.17 client.select takes &[DatumWithOid] directly (no Some)
            client.select(
                "SELECT view_name FROM aspiral.metadata WHERE parent_view = $1 ORDER BY frame_seconds ASC",
                None,
                &[DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap()
        };
        for row in tuple_table {
            let child = row.get::<String>(1).unwrap().unwrap();
            children.push(child);
        }
        Ok::<Vec<String>, spi::Error>(children)
    }).unwrap_or_default()
}
