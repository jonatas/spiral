use regex::Regex;

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
    // Replace avg(col) with sum(col) as col_sum, count(col) as col_count
    let re = Regex::new(r"avg\(([^)]+)\)").unwrap();
    re.replace_all(sql, "sum($1) as $1_sum, count($1) as $1_count").to_string()
}

pub fn child_rollup_sql(parent_name: &str, frame_seconds: i32) -> String {
    // This is a simplified version for the POC. 
    // It assumes the parent already has the expanded columns.
    format!(
        "SELECT (t / {frame_seconds}) * {frame_seconds} as t, 
                sum(price_sum) as price_sum, 
                sum(price_count) as price_count,
                max(price_max) as price_max
         FROM {parent_name}
         GROUP BY 1 ORDER BY 1"
    )
}
