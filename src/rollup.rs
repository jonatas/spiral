use pgrx::prelude::*;

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    pub seconds: i32,
}

pub const DEFAULT_FRAMES: &str = "1m,1d,1M";

pub fn parse_frames(frames_str: &str) -> Vec<Frame> {
    frames_str.split(',')
        .map(|s| {
            let s = s.trim();
            let seconds = if s.ends_with('s') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0)
            } else if s.ends_with('m') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0) * 60
            } else if s.ends_with('h') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0) * 3600
            } else if s.ends_with('d') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0) * 86400
            } else if s.ends_with('w') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0) * 604800
            } else if s.ends_with('M') {
                s[..s.len()-1].parse::<i32>().unwrap_or(0) * 2592000 // 30 days
            } else {
                s.parse::<i32>().unwrap_or(0)
            };
            let name = if s.ends_with('M') {
                format!("{}mon", &s[..s.len()-1])
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

pub fn derive_child_sql(child_name: &str, parent_name: &str, frame_seconds: i32, scope_columns: &[String]) -> (String, Vec<SourceDef>) {
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
        
        let mut select_cols = vec![format!("to_timestamp(((aspiral(t) / {0}) * {0})::double precision) as t", frame_seconds)];
        let mut group_by = vec!["(aspiral(t) / {0}) * {0}".replace("{0}", &frame_seconds.to_string())];
        let mut sources = Vec::new();
        
        let parent_is_view = client.select(
            "SELECT 1 FROM aspiral.metadata WHERE view_name = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(parent_name.into_datum().unwrap(), pg_sys::TEXTOID)] }
        )?.first().is_empty() == false;

        for row in columns {
            let col = row.get::<String>(1)?.unwrap();
            if col == "t" { continue; }
            
            if scope_columns.contains(&col) {
                select_cols.push(format!("\"{}\"", col));
                group_by.push(format!("\"{}\"", col));
                continue;
            }
            
            // Heuristic for mapping view columns back to base table columns
            let mut base_col = col.clone();
            let (agg, formula) = if col.ends_with("_stats") {
                if !parent_is_view {
                    base_col = col[..col.len()-6].to_string();
                    (format!("aspiral_stats(\"{}\") as \"{}\"", base_col, col), "stats")
                } else {
                    // Try to extract base column from sources table if it exists
                    (format!("aspiral_stats_merge(\"{}\") as \"{}\"", col, col), "stats")
                }
            } else if col.ends_with("_sum") {
                if !parent_is_view {
                    base_col = col[..col.len()-4].to_string();
                    (format!("sum(\"{}\") as \"{}\"", base_col, col), "sum")
                } else {
                    (format!("sum(\"{}\") as \"{}\"", col, col), "sum")
                }
            } else if col.ends_with("_count") {
                if !parent_is_view {
                    base_col = col[..col.len()-6].to_string();
                    (format!("count(*) as \"{}\"", col), "count")
                } else {
                    (format!("sum(\"{}\") as \"{}\"", col, col), "count")
                }
            } else if col.ends_with("_sketch") {
                if !parent_is_view {
                    base_col = col[..col.len()-7].to_string();
                    (format!("aspiral_sketch(\"{}\") as \"{}\"", base_col, col), "sketch")
                } else {
                    (format!("aspiral_sketch_merge(\"{}\") as \"{}\"", col, col), "sketch")
                }
            } else if col.ends_with("_o") {
                if !parent_is_view {
                    base_col = col[..col.len()-2].to_string();
                    (format!("first(\"{}\", aspiral(t)) as \"{}\"", base_col, col), "ohlc_o")
                } else {
                    (format!("first(\"{}\", aspiral(t)) as \"{}\"", col, col), "ohlc_o")
                }
            } else if col.ends_with("_h") {
                if !parent_is_view {
                    base_col = col[..col.len()-2].to_string();
                    (format!("max(\"{}\") as \"{}\"", base_col, col), "ohlc_h")
                } else {
                    (format!("max(\"{}\") as \"{}\"", col, col), "ohlc_h")
                }
            } else if col.ends_with("_l") {
                if !parent_is_view {
                    base_col = col[..col.len()-2].to_string();
                    (format!("min(\"{}\") as \"{}\"", base_col, col), "ohlc_l")
                } else {
                    (format!("min(\"{}\") as \"{}\"", col, col), "ohlc_l")
                }
            } else if col.ends_with("_c") {
                if !parent_is_view {
                    base_col = col[..col.len()-2].to_string();
                    (format!("last(\"{}\", aspiral(t)) as \"{}\"", base_col, col), "ohlc_c")
                } else {
                    (format!("last(\"{}\", aspiral(t)) as \"{}\"", col, col), "ohlc_c")
                }
            } else if col == "h" || col.ends_with("_max") {
                (format!("max(\"{}\") as \"{}\"", col, col), "max")
            } else if col == "l" || col.ends_with("_min") {
                (format!("min(\"{}\") as \"{}\"", col, col), "min")
            } else if col == "volume" || col == "amount" || col.ends_with("_revenue") {
                (format!("sum(\"{}\") as \"{}\"", col, col), "sum")
            } else {
                (format!("sum(\"{}\") as \"{}\"", col, col), "sum")
            };

            // If the parent is a view, we need to lookup the true base_column from aspiral.sources.
            if parent_is_view {
                if let Ok(res) = client.select("SELECT base_column FROM aspiral.sources WHERE view_name = $1 AND mat_column = $2 LIMIT 1", None, unsafe { &[
                    pgrx::datum::DatumWithOid::new(parent_name.into_datum().unwrap(), pg_sys::TEXTOID),
                    pgrx::datum::DatumWithOid::new(col.clone().into_datum().unwrap(), pg_sys::TEXTOID),
                ]}) {
                    for row in res {
                        if let Ok(Some(bc)) = row.get::<String>(1) {
                            base_col = bc;
                        }
                        break;
                    }
                }
            }

            select_cols.push(agg);
            sources.push(SourceDef {
                base_column: base_col,
                formula: formula.to_string(),
                mat_column: col.clone(),
            });
        }
        
        let scope_cols_str = scope_columns.iter().map(|s| format!("\"{}\"", s.trim())).collect::<Vec<_>>().join(", ");

        let index_sql = if scope_columns.is_empty() {
            format!("CREATE UNIQUE INDEX idx_u_{child_name} ON {child_name}(t)")
        } else {
            // Use Z-Order for clustered multi-tenant access
            format!(
                "CREATE INDEX idx_z_{child_name} ON {child_name} (
                    aspiral_zorder(aspiral(t), ARRAY[{scope_cols_str}]::text[])
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
        error!("Aspiral failed to derive child SQL for {}: {:?}", parent_name, e);
    })
}
