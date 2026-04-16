use regex::Regex;
use pgrx::prelude::*;

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    pub seconds: i32,
}

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
            } else {
                s.parse::<i32>().unwrap_or(0)
            };
            Frame { name: s.to_string(), seconds }
        })
        .filter(|f| f.seconds > 0)
        .collect()
}

pub fn expand_avg_in_sql(sql: &str) -> String {
    let re = Regex::new(r"avg\(([^)]+)\)").unwrap();
    re.replace_all(sql, "sum($1) as $1_sum, count($1) as $1_count").to_string()
}

pub fn derive_child_sql(child_name: &str, parent_name: &str, frame_seconds: i32) -> String {
    Spi::connect(|client| {
        let query = format!(
            "SELECT column_name FROM information_schema.columns WHERE table_name = '{}' AND table_schema NOT IN ('information_schema', 'pg_catalog')",
            parent_name
        );
        let columns = client.select(&query, None, &[])?;
        
        let mut agg_cols = Vec::new();
        for row in columns {
            let col = row.get::<String>(1)?.unwrap();
            if col == "t" { continue; }
            
            let agg = if col.ends_with("_max") || col == "h" {
                format!("max({}) as {}", col, col)
            } else if col.ends_with("_min") || col == "l" {
                format!("min({}) as {}", col, col)
            } else if col.ends_with("_sum") || col == "volume" || col.ends_with("_count") {
                format!("sum({}) as {}", col, col)
            } else if col.ends_with("_first") || col == "o" {
                format!("first({}) as {}", col, col)
            } else if col.ends_with("_last") || col == "c" {
                format!("last({}) as {}", col, col)
            } else {
                format!("last({}) as {}", col, col)
            };
            agg_cols.push(agg);
        }
        
        let sql = format!(
            "CREATE MATERIALIZED VIEW {child_name} AS 
             SELECT (t / {frame_seconds}) * {frame_seconds} as t, {agg_cols} 
             FROM {parent_name}
             WHERE t < ((aspiral_now()::{aspiral_type} / {frame_seconds}) * {frame_seconds})
             GROUP BY 1",
            child_name = child_name,
            agg_cols = agg_cols.join(", "), 
            parent_name = parent_name,
            frame_seconds = frame_seconds,
            aspiral_type = "aspiral"
        );
        Ok::<String, spi::Error>(sql)
    }).unwrap_or_else(|e| {
        error!("Aspiral failed to derive child SQL for {}: {:?}", parent_name, e);
    })
}
