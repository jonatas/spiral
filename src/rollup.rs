use pgrx::prelude::*;

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    pub seconds: i32,
}

pub const DEFAULT_FRAMES: &str = "1m,1d,1M";

/// Parses a comma-separated string of time frames into a vector of `Frame` structs.
///
/// This function understands suffixes like `s` (seconds), `m` (minutes), `h` (hours), 
/// `d` (days), `w` (weeks), and `M` (months of 30 days).
///
/// # Examples
///
/// ```rust
/// use spiral::rollup::parse_frames;
///
/// let frames = parse_frames("1m, 1h, 1M");
/// 
/// assert_eq!(frames.len(), 3);
/// assert_eq!(frames[0].seconds, 60); // 1 minute
/// assert_eq!(frames[1].seconds, 3600); // 1 hour
/// assert_eq!(frames[2].seconds, 2592000); // 30 days
///
/// // Invalid or zero cases are ignored
/// let invalid = parse_frames("0s, -1m, abc");
/// assert_eq!(invalid.len(), 0);
/// ```
pub fn parse_frames(frames_str: &str) -> Vec<Frame> {
    frames_str
        .split(',')
        .map(|s| {
            let s = s.trim();
            let seconds = if let Some(stripped) = s.strip_suffix('s') {
                stripped.parse::<i32>().unwrap_or(0)
            } else if let Some(stripped) = s.strip_suffix('m') {
                stripped.parse::<i32>().unwrap_or(0) * 60
            } else if let Some(stripped) = s.strip_suffix('h') {
                stripped.parse::<i32>().unwrap_or(0) * 3600
            } else if let Some(stripped) = s.strip_suffix('d') {
                stripped.parse::<i32>().unwrap_or(0) * 86400
            } else if let Some(stripped) = s.strip_suffix('w') {
                stripped.parse::<i32>().unwrap_or(0) * 604800
            } else if let Some(stripped) = s.strip_suffix('M') {
                stripped.parse::<i32>().unwrap_or(0) * 2592000 // 30 days
            } else {
                s.parse::<i32>().unwrap_or(0)
            };
            let name = if let Some(stripped) = s.strip_suffix('M') {
                format!("{}mon", stripped)
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
    pub rollup_gsub_strategy: Option<String>,
}

pub fn derive_child_sql(
    child_name: &str,
    parent_name: &str,
    frame_seconds: i32,
    scope_columns: &[String],
) -> (String, Vec<SourceDef>) {
    Spi::connect(|client| {
        let exists_res = client.select(
            "SELECT 1 FROM pg_class WHERE relname = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(child_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        );
        let exists = match exists_res {
            Ok(t) => !t.is_empty(),
            Err(_) => false,
        };

        let mut select_cols = vec![format!("to_timestamp(((spiral(t) / {0}) * {0} + 946684800)::double precision) as t", frame_seconds)];
        let mut group_by = vec!["(spiral(t) / {0}) * {0}".replace("{0}", &frame_seconds.to_string())];
        let mut sources = Vec::new();

        for s in scope_columns {
            select_cols.push(format!("\"{}\"", s));
            group_by.push(format!("\"{}\"", s));
        }

        let parent_is_view_res = client.select(
            "SELECT frame_seconds FROM spiral.metadata WHERE view_name = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(parent_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        );
        let parent_is_view = match parent_is_view_res {
            Ok(t) => {
                if t.is_empty() {
                    false
                } else {
                    t.first().get::<i32>(1).unwrap().unwrap_or(0) > 0
                }
            },
            Err(_) => false,
        };

        let sources_query = format!("SELECT base_column, formula, mat_column, rollup_gsub_strategy FROM spiral.sources WHERE view_name = '{}'", child_name.replace("'", "''"));
        let mut loaded_sources = Vec::new();
        let mut loaded_from_parent = false;

        if exists {
            if let Ok(res) = client.select(&sources_query, None, &[]) {
                for row in res {
                    if let (Ok(Some(bc)), Ok(Some(f)), Ok(Some(mc)), Ok(rgs)) = (row.get::<String>(1), row.get::<String>(2), row.get::<String>(3), row.get::<String>(4)) {
                        loaded_sources.push(SourceDef { base_column: bc, formula: f, mat_column: mc, rollup_gsub_strategy: rgs });
                    }
                }
            }
        }

        if loaded_sources.is_empty() {
            let parent_sources_query = format!("SELECT base_column, formula, mat_column, rollup_gsub_strategy FROM spiral.sources WHERE view_name = '{}'", parent_name.replace("'", "''"));
            if let Ok(res) = client.select(&parent_sources_query, None, &[]) {
                for row in res {
                    if let (Ok(Some(bc)), Ok(Some(f)), Ok(Some(mc)), Ok(rgs)) = (row.get::<String>(1), row.get::<String>(2), row.get::<String>(3), row.get::<String>(4)) {
                        loaded_sources.push(SourceDef { base_column: bc, formula: f, mat_column: mc, rollup_gsub_strategy: rgs });
                        loaded_from_parent = true;
                    }
                }
            }
        }

        if !loaded_sources.is_empty() {
            if loaded_from_parent {
                sources = loaded_sources.clone();
            } else {
                sources = loaded_sources;
            }
            
            for src in &sources {
                if let Some(strategy) = &src.rollup_gsub_strategy {
                    let col_ident = if parent_is_view { &src.mat_column } else { &src.base_column };
                    let sql = strategy.replace("rollup(\"\\1\")", &format!("\"{}\"", col_ident))
                                      .replace("\\1", &src.mat_column);
                    select_cols.push(sql);
                } else if src.formula == "stats" {
                    if !parent_is_view {
                        select_cols.push(format!("spiral_stats(\"{}\") as \"{}\"", src.base_column, src.mat_column));
                    } else {
                        select_cols.push(format!("spiral_stats_merge(\"{}\") as \"{}\"", src.mat_column, src.mat_column));
                    }
                } else if src.formula == "sketch" {
                    if !parent_is_view {
                        select_cols.push(format!("spiral_sketch(\"{}\") as \"{}\"", src.base_column, src.mat_column));
                    } else {
                        select_cols.push(format!("spiral_sketch_merge(\"{}\") as \"{}\"", src.mat_column, src.mat_column));
                    }
                } else if src.formula == "ohlcv" {
                    let bc = &src.base_column;
                    let mc = &src.mat_column;
                    if !parent_is_view {
                        select_cols.push(format!("first(\"{}\", spiral(t)) as \"{}_o\", max(\"{}\") as \"{}_h\", min(\"{}\") as \"{}_l\", last(\"{}\", spiral(t)) as \"{}_c\", sum(\"{}\") as \"{}_v\"", bc, mc, bc, mc, bc, mc, bc, mc, bc, mc));
                    } else {
                        select_cols.push(format!("first(\"{}_o\", spiral(t)) as \"{}_o\", max(\"{}_h\") as \"{}_h\", min(\"{}_l\") as \"{}_l\", last(\"{}_c\", spiral(t)) as \"{}_c\", sum(\"{}_v\") as \"{}_v\"", mc, mc, mc, mc, mc, mc, mc, mc, mc, mc));
                    }
                } else {
                    let bc = if parent_is_view { &src.mat_column } else { &src.base_column };
                    select_cols.push(format!("sum(\"{}\") as \"{}\"", bc, src.mat_column));
                }
            }
        } else {
            let source_for_cols = if exists { child_name } else { parent_name };
            let query = format!(
                "SELECT a.attname::text
                 FROM pg_attribute a
                 JOIN pg_class c ON a.attrelid = c.oid
                 WHERE c.relname = '{}' AND a.attnum > 0 AND NOT attisdropped",
                source_for_cols.replace("'", "''")
            );
            if let Ok(columns) = client.select(&query, None, &[]) {
                for row in columns {
                    let col = row.get::<String>(1).unwrap().unwrap();
                    if col == "t" || scope_columns.contains(&col) { continue; }

                    let base_col = col.clone();
                    select_cols.push(format!("sum(\"{}\") as \"{}\"", col, col));
                    sources.push(SourceDef { base_column: base_col, formula: "sum".to_string(), mat_column: col.clone(), rollup_gsub_strategy: None });
                }
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

