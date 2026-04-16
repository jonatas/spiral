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
        created_at timestamptz DEFAULT now()
    );

    CREATE TABLE IF NOT EXISTS aspiral.changelog (
        base_view text NOT NULL,
        bucket_t bigint NOT NULL,
        PRIMARY KEY (base_view, bucket_t)
    );
    "#,
    name = "create_aspiral_metadata"
);

pub fn mark_bucket_dirty(base_view: &str, t: i64) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.changelog (base_view, bucket_t) VALUES ($1, $2) ON CONFLICT DO NOTHING",
            &[
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(t.into_datum(), PgBuiltInOids::INT8OID.value()),
            ],
        ).unwrap();
    }
}

pub fn get_dirty_buckets(base_view: &str) -> Vec<i64> {
    Spi::connect(|client| {
        let mut buckets = Vec::new();
        let tuple_table = unsafe {
            client.select(
                "SELECT bucket_t FROM aspiral.changelog WHERE base_view = $1",
                None,
                &[DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value())]
            ).unwrap()
        };
        for row in tuple_table {
            let t = row.get::<i64>(1).unwrap().unwrap();
            buckets.push(t);
        }
        Ok::<Vec<i64>, spi::Error>(buckets)
    }).unwrap_or_default()
}

pub fn clear_dirty_buckets(base_view: &str, buckets: &[i64]) {
    if buckets.is_empty() { return; }
    // Using a simple DELETE for the POC
    let t_list = buckets.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",");
    Spi::run(&format!("DELETE FROM aspiral.changelog WHERE base_view = '{}' AND bucket_t IN ({})", base_view, t_list)).unwrap();
}

pub fn insert_metadata(view_name: &str, parent_view: &str, frame_seconds: i32, base_view: &str) {
    unsafe {
        Spi::run_with_args(
            "INSERT INTO aspiral.metadata (view_name, parent_view, frame_seconds, base_view) 
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (view_name) DO UPDATE SET parent_view = EXCLUDED.parent_view, frame_seconds = EXCLUDED.frame_seconds, base_view = EXCLUDED.base_view",
            &[
                DatumWithOid::new(view_name.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(parent_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
                DatumWithOid::new(frame_seconds.into_datum(), PgBuiltInOids::INT4OID.value()),
                DatumWithOid::new(base_view.into_datum(), PgBuiltInOids::TEXTOID.value()),
            ],
        ).unwrap();
    }
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
