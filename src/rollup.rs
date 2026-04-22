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

pub fn derive_child_sql(child_name: &str, parent_name: &str, frame_seconds: i32, scope_columns: &[String]) -> String {
    Spi::connect(|client| {
        let exists = client.select(
            "SELECT 1 FROM pg_class WHERE relname = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(child_name.into_datum(), pg_sys::TEXTOID)] }
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
        
        let mut select_cols = vec![format!("to_timestamptz((aspiral(t) / {0}) * {0}) as t", frame_seconds)];
        let mut group_by = vec!["(aspiral(t) / {0}) * {0}".replace("{0}", &frame_seconds.to_string())];
        
        let parent_is_view = client.select(
            "SELECT 1 FROM aspiral.metadata WHERE view_name = $1",
            Some(1),
            unsafe { &[pgrx::datum::DatumWithOid::new(parent_name.into_datum(), pg_sys::TEXTOID)] }
        )?.first().is_empty() == false;

        for row in columns {
            let col = row.get::<String>(1)?.unwrap();
            if col == "t" { continue; }
            
            if scope_columns.contains(&col) {
                select_cols.push(col.clone());
                group_by.push(col.clone());
                continue;
            }
            
            // Heuristic for mapping view columns back to base table columns
            let agg = if col.ends_with("_stats") {
                if !parent_is_view {
                    let base = &col[..col.len()-6];
                    format!("aspiral_stats(\"{}\") as \"{}\"", base, col)
                } else {
                    format!("aspiral_stats_merge(\"{}\") as \"{}\"", col, col)
                }
            } else if col.ends_with("_sum") {
                if !parent_is_view {
                    let base = &col[..col.len()-4];
                    format!("sum(\"{}\") as \"{}\"", base, col)
                } else {
                    format!("sum(\"{}\") as \"{}\"", col, col)
                }
            } else if col.ends_with("_sketch") {
                if !parent_is_view {
                    let base = &col[..col.len()-7];
                    format!("aspiral_sketch(\"{}\") as \"{}\"", base, col)
                } else {
                    format!("aspiral_sketch_merge(\"{}\") as \"{}\"", col, col)
                }
            } else if col.ends_with("_o") {
                if !parent_is_view {
                    let base = &col[..col.len()-2];
                    format!("first(\"{}\", aspiral(t)) as \"{}\"", base, col)
                } else {
                    format!("first(\"{}\", aspiral(t)) as \"{}\"", col, col)
                }
            } else if col.ends_with("_h") {
                if !parent_is_view {
                    let base = &col[..col.len()-2];
                    format!("max(\"{}\") as \"{}\"", base, col)
                } else {
                    format!("max(\"{}\") as \"{}\"", col, col)
                }
            } else if col.ends_with("_l") {
                if !parent_is_view {
                    let base = &col[..col.len()-2];
                    format!("min(\"{}\") as \"{}\"", base, col)
                } else {
                    format!("min(\"{}\") as \"{}\"", col, col)
                }
            } else if col.ends_with("_c") {
                if !parent_is_view {
                    let base = &col[..col.len()-2];
                    format!("last(\"{}\", aspiral(t)) as \"{}\"", base, col)
                } else {
                    format!("last(\"{}\", aspiral(t)) as \"{}\"", col, col)
                }
            } else if col == "h" || col.ends_with("_max") {
                format!("max(\"{}\") as \"{}\"", col, col)
            } else if col == "l" || col.ends_with("_min") {
                format!("min(\"{}\") as \"{}\"", col, col)
            } else if col == "volume" || col == "amount" || col.ends_with("_revenue") {
                format!("sum(\"{}\") as \"{}\"", col, col)
            } else {
                format!("last(\"{}\", aspiral(t)) as \"{}\"", col, col)
            };
            select_cols.push(agg);
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

        Ok::<String, spi::Error>(sql)
    }).unwrap_or_else(|e| {
        error!("Aspiral failed to derive child SQL for {}: {:?}", parent_name, e);
    })
}
