use pgrx::prelude::*;

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    pub seconds: i32,
}

pub const DEFAULT_FRAMES: &str = "1m,1d,1M";

pub fn parse_frames(frames_str: &str) -> Vec<Frame> {
    frames_str
        .split(',')
        .map(|s| {
            let s = s.trim();
            let seconds = if s.ends_with('s') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0)
            } else if s.ends_with('m') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0) * 60
            } else if s.ends_with('h') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0) * 3600
            } else if s.ends_with('d') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0) * 86400
            } else if s.ends_with('w') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0) * 604800
            } else if s.ends_with('M') {
                s[..s.len() - 1].parse::<i32>().unwrap_or(0) * 2592000 // 30 days
            } else {
                s.parse::<i32>().unwrap_or(0)
            };
            let name = if s.ends_with('M') {
                format!("{}mon", &s[..s.len() - 1])
            } else {
                s.to_string()
            };
            Frame { name, seconds }
        })
        .filter(|f| f.seconds > 0)
        .collect()
}

#[derive(Debug, Clone)]
pub struct SourceDef {
    pub base_column: String,
    pub formula: String,
    pub mat_column: String,
}

pub fn derive_child_sql(
    child_name: &str,
    parent_name: &str,
    frame_seconds: i32,
    scope_columns: &[String],
) -> (String, Vec<SourceDef>) {
    Spi::connect(|client| {
        let exists = client.select(
            "SELECT 1 FROM pg_class WHERE relname = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(child_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        )?.first().is_empty() == false;

        let source_for_cols = if exists { child_name } else { parent_name };

        let query = format!(
            "SELECT a.attname::text
             FROM pg_attribute a
             JOIN pg_class c ON a.attrelid = c.oid
             WHERE c.relname = '{}' AND a.attnum > 0 AND NOT attisdropped",
            source_for_cols.replace("'", "''")
        );
        let columns = client.select(&query, None, &[])?;

        let mut select_cols = vec![format!("to_timestamp(((spiral(t) / {0}) * {0})::double precision) as t", frame_seconds)];
        let mut group_by = vec!["(spiral(t) / {0}) * {0}".replace("{0}", &frame_seconds.to_string())];
        let mut sources = Vec::new();

        let parent_is_view = client.select(
            "SELECT 1 FROM spiral.metadata WHERE view_name = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(parent_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        )?.first().is_empty() == false;

        for row in columns {
            let col = row.get::<String>(1)?.unwrap();
            if col == "t" { continue; }

            if scope_columns.contains(&col) {
                if !select_cols.iter().any(|s| s.contains(&format!("\"{}\"", col))) {
                    select_cols.push(format!("\"{}\"", col));
                    group_by.push(format!("\"{}\"", col));
                }
                continue;
            }

            // Heuristic for mapping view columns back to base table columns
            let base_col: String;
            if col.ends_with("_stats") {
                if !parent_is_view {
                    base_col = col[..col.len()-6].to_string();
                    select_cols.push(format!("spiral_stats(\"{}\") as \"{}\"", base_col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "stats".to_string(), mat_column: col.clone() });
                } else {
                    base_col = col.clone();
                    select_cols.push(format!("spiral_stats_merge(\"{}\") as \"{}\"", col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "stats".to_string(), mat_column: col.clone() });
                }
            } else if col.ends_with("_sketch") {
                if !parent_is_view {
                    base_col = col[..col.len()-7].to_string();
                    select_cols.push(format!("spiral_sketch(\"{}\") as \"{}\"", base_col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "sketch".to_string(), mat_column: col.clone() });
                } else {
                    base_col = col.clone();
                    select_cols.push(format!("spiral_sketch_merge(\"{}\") as \"{}\"", col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "sketch".to_string(), mat_column: col.clone() });
                }
            } else if col.ends_with("_ohlcv") {
                if !parent_is_view {
                    base_col = col[..col.len()-6].to_string();
                    select_cols.push(format!("first(\"{}\", spiral(t)) as \"{}_o\", max(\"{}\") as \"{}_h\", min(\"{}\") as \"{}_l\", last(\"{}\", spiral(t)) as \"{}_c\", sum(\"{}\") as \"{}_v\"", base_col, col, base_col, col, base_col, col, base_col, col, base_col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "ohlcv".to_string(), mat_column: col.clone() });
                } else {
                    base_col = col.clone();
                    select_cols.push(format!("first(\"{}_o\", spiral(t)) as \"{}_o\", max(\"{}_h\") as \"{}_h\", min(\"{}_l\") as \"{}_l\", last(\"{}_c\", spiral(t)) as \"{}_c\", sum(\"{}_v\") as \"{}_v\"", col, col, col, col, col, col, col, col, col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "ohlcv".to_string(), mat_column: col.clone() });
                }
            } else {
                base_col = col.clone();
                // Default to sum for other columns
                select_cols.push(format!("sum(\"{}\") as \"{}\"", col, col));
                sources.push(SourceDef { base_column: base_col, formula: "sum".to_string(), mat_column: col.clone() });
            }
        }

        let scope_cols_str = scope_columns.iter().map(|s| format!("\"{}\"", s.trim())).collect::<Vec<_>>().join(", ");

        let index_sql = if scope_columns.is_empty() {
            format!("CREATE UNIQUE INDEX IF NOT EXISTS idx_u_{child_name} ON {child_name}(t)")
        } else {
            // Use Z-Order for clustered multi-tenant access
            format!(
                "CREATE INDEX IF NOT EXISTS idx_z_{child_name} ON {child_name} (
                    spiral_zorder(spiral(t), ARRAY[{scope_cols_str}]::text[])
                )"
            )
        };

        let sql = format!(
            "CREATE TABLE {child_name} AS
             SELECT {select_cols}
             FROM {parent_name}
             GROUP BY {group_by};
             {index_sql};",
            child_name = child_name,
            select_cols = select_cols.join(", "),
            parent_name = parent_name,
            group_by = group_by.join(", "),
            index_sql = index_sql
        );

        Ok::< (String, Vec<SourceDef>), spi::Error>((sql, sources))
    }).unwrap_or_else(|e| {
        error!("Spiral failed to derive child SQL for {}: {:?}", parent_name, e);
    })
}
