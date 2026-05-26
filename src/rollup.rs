use pgrx::prelude::*;

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    pub seconds: i32,
    /// Non-None for calendar-aligned tiers (month, quarter, year).
    /// Holds the `date_trunc` field name (e.g. "month", "quarter", "year").
    /// When set, rollup SQL uses `date_trunc(field, t)` instead of epoch
    /// arithmetic so bucket boundaries align with calendar boundaries.
    pub calendar_field: Option<String>,
}

pub const DEFAULT_FRAMES: &str = "1m,1d,1M";

/// Parses a comma-separated string of time frames into a vector of `Frame` structs.
///
/// Understands suffixes: `s` (seconds), `m` (minutes), `h` (hours), `d` (days),
/// `w` (weeks), `M` (calendar month), `Q` (calendar quarter), `Y` (calendar year).
/// Also accepts word suffixes: `mon`/`month`, `quarter`, `year`.
///
/// Calendar frames (`M`, `Q`, `Y`) use `date_trunc` for bucket alignment so that
/// boundaries fall on the first of the month / quarter / year rather than on
/// fixed 30-day epoch multiples.
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
/// assert_eq!(frames[2].seconds, 2592000); // approx 30 days (calendar-aligned)
/// assert!(frames[2].calendar_field.is_some());
///
/// // Word aliases work too
/// let frames2 = parse_frames("1h,1d,1mon");
/// assert_eq!(frames2.len(), 3);
/// assert!(frames2[2].calendar_field.is_some());
///
/// // Invalid or zero cases are ignored
/// let invalid = parse_frames("0s, -1m, abc");
/// assert_eq!(invalid.len(), 0);
/// ```
pub fn parse_frames(frames_str: &str) -> Vec<Frame> {
    frames_str
        .split(',')
        .filter_map(|s| {
            let s = s.trim();

            // Calendar word suffixes (check before single-char suffixes)
            if s.ends_with("month") || s.ends_with("mon") || s.ends_with("mo") {
                let n: i32 = s
                    .trim_end_matches("month")
                    .trim_end_matches("mon")
                    .trim_end_matches("mo")
                    .parse()
                    .unwrap_or(0);
                if n <= 0 {
                    return None;
                }
                return Some(Frame {
                    name: format!("{}mo", n),
                    seconds: n * 2_592_000,
                    calendar_field: Some("month".to_string()),
                });
            }
            if s.ends_with("quarter") {
                let n: i32 = s.trim_end_matches("quarter").parse().unwrap_or(0);
                if n <= 0 {
                    return None;
                }
                return Some(Frame {
                    name: format!("{}quarter", n),
                    seconds: n * 7_776_000,
                    calendar_field: Some("quarter".to_string()),
                });
            }
            if s.ends_with("year") || s.ends_with('Y') {
                let n: i32 = s
                    .trim_end_matches("year")
                    .trim_end_matches('Y')
                    .parse()
                    .unwrap_or(0);
                if n <= 0 {
                    return None;
                }
                return Some(Frame {
                    name: format!("{}year", n),
                    seconds: n * 31_536_000,
                    calendar_field: Some("year".to_string()),
                });
            }

            // Single-char and numeric suffixes
            let (seconds, name, cal) = if let Some(stripped) = s.strip_suffix('M') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n * 2_592_000, format!("{}mo", n), Some("month".to_string()))
            } else if let Some(stripped) = s.strip_suffix('Q') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (
                    n * 7_776_000,
                    format!("{}quarter", n),
                    Some("quarter".to_string()),
                )
            } else if let Some(stripped) = s.strip_suffix('s') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n, s.to_string(), None)
            } else if let Some(stripped) = s.strip_suffix('m') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n * 60, s.to_string(), None)
            } else if let Some(stripped) = s.strip_suffix('h') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n * 3600, s.to_string(), None)
            } else if let Some(stripped) = s.strip_suffix('d') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n * 86400, s.to_string(), None)
            } else if let Some(stripped) = s.strip_suffix('w') {
                let n = stripped.parse::<i32>().unwrap_or(0);
                (n * 604_800, s.to_string(), None)
            } else {
                let n = s.parse::<i32>().unwrap_or(0);
                (n, s.to_string(), None)
            };

            if seconds <= 0 {
                None
            } else {
                Some(Frame {
                    name,
                    seconds,
                    calendar_field: cal,
                })
            }
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct SourceDef {
    pub base_column: String,
    pub formula: String,
    pub mat_column: String,
    pub rollup_gsub_strategy: Option<String>,
}

/// Returns the `date_trunc` field for a frame given its size in seconds,
/// or `None` for fixed-second frames that don't need calendar alignment.
pub fn calendar_field_for_seconds(frame_seconds: i32) -> Option<&'static str> {
    match frame_seconds {
        2_592_000 => Some("month"),
        7_776_000 => Some("quarter"),
        31_536_000 => Some("year"),
        _ => None,
    }
}

pub fn derive_child_sql(
    child_name: &str,
    parent_name: &str,
    frame_seconds: i32,
    scope_columns: &[String],
    calendar_field: Option<&str>,
) -> (String, Vec<SourceDef>) {
    Spi::connect(|client| {
        let exists_res = client.select(
            &format!("SELECT 1 FROM pg_class WHERE relname = '{}'", child_name.replace("'", "''")),
            Some(1),
            &[]
        );
        let exists = match exists_res {
            Ok(t) => !t.is_empty(),
            Err(_) => false,
        };

        let parent_is_view_res = client.select(
            &format!("SELECT frame_seconds FROM spiral.metadata WHERE view_name = '{}'", parent_name.replace("'", "''")),
            Some(1),
            &[]
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

        let (anchor_col, _offset_cols) = if !parent_is_view {
            let metadata_res = client.select(&format!("SELECT columns_metadata FROM spiral.metadata WHERE view_name = '{}'", parent_name.replace("'", "''")), Some(1), &[]);
            if let Ok(m) = metadata_res {
                if !m.is_empty() {
                    let json: pgrx::JsonB = m.first().get(1).unwrap().unwrap();
                    let anchor = json.0.get("time_column").and_then(|v| v.as_str()).unwrap_or("t").to_string();
                    let offsets: Vec<String> = json.0.get("offset_columns")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().map(|v| v.as_str().unwrap().to_string()).collect())
                        .unwrap_or_default();
                    (anchor, offsets)
                } else { ("t".to_string(), Vec::new()) }
            } else { ("t".to_string(), Vec::new()) }
        } else { ("t".to_string(), Vec::new()) };

        let (time_select, time_group) = if let Some(field) = calendar_field {
            // Calendar-aligned tier: snap to month/quarter/year boundaries.
            (
                format!("date_trunc('{}', \"{}\") as t", field, anchor_col),
                format!("date_trunc('{}', \"{}\")", field, anchor_col),
            )
        } else {
            (
                format!(
                    "to_timestamp(((spiral(\"{0}\") / {1}) * {1})::double precision) as t",
                    anchor_col, frame_seconds
                ),
                format!("(spiral(\"{}\") / {1}) * {1}", anchor_col, frame_seconds),
            )
        };
        let mut select_cols = vec![time_select];
        let mut group_by = vec![time_group];
        let mut sources = Vec::new();

        for s in scope_columns {
            select_cols.push(format!("\"{}\"", s));
            group_by.push(format!("\"{}\"", s));
        }

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
                } else if src.formula == "tdigest" {
                    if !parent_is_view {
                        select_cols.push(format!("spiral_tdigest(\"{}\") as \"{}\"", src.base_column, src.mat_column));
                    } else {
                        select_cols.push(format!("spiral_tdigest_merge(\"{}\") as \"{}\"", src.mat_column, src.mat_column));
                    }
                } else if src.formula == "ohlcv" {
                    if !parent_is_view {
                        select_cols.push(format!("spiral_ohlcv(\"{}\", spiral(t)) as \"{}\"", src.base_column, src.mat_column));
                    } else {
                        select_cols.push(format!("spiral_ohlcv_merge(\"{}\") as \"{}\"", src.mat_column, src.mat_column));
                    }
                } else if src.formula == "range_max_end" || src.formula == "range_merge" {
                    if !parent_is_view {
                        let bucket_expr = format!(
                            "to_timestamp(((spiral(\"{0}\") / {1}) * {1})::double precision)",
                            anchor_col, frame_seconds
                        );
                        select_cols.push(format!(
                            "date_part('epoch', max(\"{}\") - {})::int4 as \"{}\"",
                            src.base_column, bucket_expr, src.mat_column
                        ));
                    } else {
                        select_cols.push(format!("max(\"{}\") as \"{}\"", src.mat_column, src.mat_column));
                    }
                } else {
                    let bc = if parent_is_view { &src.mat_column } else { &src.base_column };
                    // min/max must propagate with their own aggregate, not sum.
                    // count propagates as sum-of-counts when rolling up across tiers.
                    let agg = match src.formula.as_str() {
                        "min" => "min",
                        "max" => "max",
                        "count" if parent_is_view => "sum",
                        "count" => "count",
                        _ => "sum",
                    };
                    select_cols.push(format!("{}(\"{}\") as \"{}\"", agg, bc, src.mat_column));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frames_basic() {
        let frames = parse_frames("1m,1h,1d");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].seconds, 60);
        assert_eq!(frames[1].seconds, 3600);
        assert_eq!(frames[2].seconds, 86400);
        assert!(frames.iter().all(|f| f.calendar_field.is_none()));
    }

    #[test]
    fn test_parse_frames_capital_m_month() {
        let frames = parse_frames("1h,1d,1M");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[2].seconds, 2_592_000);
        assert_eq!(frames[2].name, "1mo");
        assert_eq!(frames[2].calendar_field.as_deref(), Some("month"));
    }

    #[test]
    fn test_parse_frames_word_suffixes() {
        let frames = parse_frames("1h,1d,1mon");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[2].seconds, 2_592_000);
        assert_eq!(frames[2].name, "1mo");
        assert_eq!(frames[2].calendar_field.as_deref(), Some("month"));

        let frames2 = parse_frames("1h,1d,1month");
        assert_eq!(frames2.len(), 3);
        assert_eq!(frames2[2].name, "1mo");
        assert_eq!(frames2[2].calendar_field.as_deref(), Some("month"));
    }

    #[test]
    fn test_parse_frames_quarter_and_year() {
        let frames = parse_frames("1h,1Q,1Y");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[1].seconds, 7_776_000);
        assert_eq!(frames[1].calendar_field.as_deref(), Some("quarter"));
        assert_eq!(frames[2].seconds, 31_536_000);
        assert_eq!(frames[2].calendar_field.as_deref(), Some("year"));
    }

    #[test]
    fn test_parse_frames_invalid_ignored() {
        let frames = parse_frames("0s,-1m,abc");
        assert_eq!(frames.len(), 0);
    }

    #[test]
    fn test_calendar_field_for_seconds() {
        assert_eq!(calendar_field_for_seconds(2_592_000), Some("month"));
        assert_eq!(calendar_field_for_seconds(7_776_000), Some("quarter"));
        assert_eq!(calendar_field_for_seconds(31_536_000), Some("year"));
        assert_eq!(calendar_field_for_seconds(3600), None);
        assert_eq!(calendar_field_for_seconds(86400), None);
    }
}
