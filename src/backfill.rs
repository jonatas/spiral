use crate::stats::StatsState;
use pgrx::prelude::*;
use std::collections::HashMap;

const FETCH_BATCH: i64 = 10_000;

/// Backfills a Welford stats rollup tier directly from raw rows, bypassing
/// `spiral_stats_accum` (see `stats.rs`), which bincode round-trips its bytea
/// state on every single-row transition call. At bulk-load volumes that
/// per-row FFI/serde cost dominates (observed ~400 rows/s on a 1.1M-row
/// backfill). This accumulates natively in a Rust `HashMap` keyed by
/// (scope values, bucket) and only encodes to bytea once per output row.
///
/// `tier_table` must already exist with columns matching `scope_cols` (any
/// type — read back as text), a `bucket` bigint column, and a `stats_col`
/// bytea column holding the bincode-encoded `StatsState`.
#[pg_extern]
pub fn spiral_bulk_backfill_stats(
    base_view: &str,
    tier_table: &str,
    frame_seconds: i64,
    scope_cols: Vec<String>,
    value_col: &str,
    stats_col: &str,
) -> i64 {
    let cursor_sql = build_scan_sql(base_view, frame_seconds, &scope_cols, value_col);
    let n_scopes = scope_cols.len();
    let mut acc: HashMap<(Vec<String>, i64), StatsState> = HashMap::new();

    // Named cursor, detached and re-found across separate Spi::connect
    // sessions: pgrx keeps every SpiTupleTable returned by fetch() alive
    // until its *session* (the Spi::connect closure) ends, so looping
    // fetch() inside one session would hold every batch in memory at
    // once — defeating the point of chunked reads. A fresh session per
    // batch lets earlier batches free before the next one is pulled.
    let cursor_name = Spi::connect(|client| {
        let cursor = client.open_cursor(&cursor_sql, &[]);
        Ok::<String, spi::Error>(cursor.detach_into_name())
    })
    .unwrap();

    loop {
        let exhausted = Spi::connect(|client| {
            let mut cursor = client.find_cursor(&cursor_name)?;
            let batch = cursor.fetch(FETCH_BATCH)?;
            let exhausted = batch.len() < FETCH_BATCH as usize;
            for row in batch {
                let mut scope_key = Vec::with_capacity(n_scopes);
                for i in 1..=n_scopes {
                    scope_key.push(row.get::<String>(i)?.unwrap_or_default());
                }
                let bucket = row.get::<i64>(n_scopes + 1)?.unwrap_or(0);
                let val = row.get::<f64>(n_scopes + 2)?.unwrap_or(0.0);
                acc.entry((scope_key, bucket)).or_default().add(val);
            }
            let _ = cursor.detach_into_name();
            Ok::<bool, spi::Error>(exhausted)
        })
        .unwrap();
        if exhausted {
            break;
        }
    }

    // Close the portal explicitly rather than leaving it detached until the
    // end of the transaction.
    Spi::connect(|client| {
        let cursor = client.find_cursor(&cursor_name)?;
        drop(cursor);
        Ok::<(), spi::Error>(())
    })
    .unwrap();

    let rows_out = acc.len() as i64;
    if rows_out == 0 {
        return 0;
    }

    write_groups(tier_table, &scope_cols, stats_col, acc);
    rows_out
}

fn build_scan_sql(base_view: &str, frame_seconds: i64, scope_cols: &[String], value_col: &str) -> String {
    let scope_select = scope_cols
        .iter()
        .map(|c| format!("\"{}\"::text", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let select_list = if scope_cols.is_empty() {
        format!(
            "(extract(epoch from t)::bigint / {}) AS bucket, \"{}\"::float8 AS val",
            frame_seconds,
            value_col.replace('"', "\"\"")
        )
    } else {
        format!(
            "{}, (extract(epoch from t)::bigint / {}) AS bucket, \"{}\"::float8 AS val",
            scope_select,
            frame_seconds,
            value_col.replace('"', "\"\"")
        )
    };
    format!(
        "SELECT {} FROM \"{}\"",
        select_list,
        base_view.replace('"', "\"\"")
    )
}

fn write_groups(
    tier_table: &str,
    scope_cols: &[String],
    stats_col: &str,
    acc: HashMap<(Vec<String>, i64), StatsState>,
) {
    let scope_cols_quoted = scope_cols
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let col_list = if scope_cols.is_empty() {
        format!("bucket, \"{}\"", stats_col.replace('"', "\"\""))
    } else {
        format!("{}, bucket, \"{}\"", scope_cols_quoted, stats_col.replace('"', "\"\""))
    };

    let values_sql: Vec<String> = acc
        .into_iter()
        .map(|((scopes, bucket), state)| {
            let stats_hex = to_hex(&crate::stats::to_binary(&state));
            if scopes.is_empty() {
                format!("({}, '\\x{}'::bytea)", bucket, stats_hex)
            } else {
                let scope_vals = scopes
                    .iter()
                    .map(|s| format!("'{}'", s.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({}, {}, '\\x{}'::bytea)", scope_vals, bucket, stats_hex)
            }
        })
        .collect();

    // One INSERT per output group would reintroduce the same per-row
    // overhead this function exists to avoid; batch instead.
    for chunk in values_sql.chunks(5_000) {
        let sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES {}",
            tier_table.replace('"', "\"\""),
            col_list,
            chunk.join(", ")
        );
        Spi::run(&sql).unwrap();
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
