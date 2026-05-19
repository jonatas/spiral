use crate::catalog;
use pgrx::prelude::*;
use serde_json::{json, Map, Value};

#[pg_extern]
pub fn spiral_validate(
    view_name: &str,
    t_start: pgrx::datum::TimestampWithTimeZone,
    t_end: pgrx::datum::TimestampWithTimeZone,
) -> pgrx::JsonB {
    let meta = match catalog::get_metadata(view_name) {
        Some(m) => m,
        None => {
            return pgrx::JsonB(
                json!({"error": format!("View '{}' not found in spiral metadata", view_name)}),
            )
        }
    };

    let base_view = &meta.base_view;
    let mut results = Map::new();

    crate::SKIP_ACCELERATION.with(|s| s.set(true));
    let final_res = if view_name == base_view {
        // If called on base view, find all rollups and validate them
        let rollups = Spi::connect(|client| {
            let sql = format!("SELECT view_name FROM spiral.metadata WHERE base_view = '{}' AND view_name != '{}'", base_view.replace("'", "''"), base_view.replace("'", "''"));
            client.select(&sql, None, &[]).map(|r| r.map(|row| row.get::<String>(1).unwrap().unwrap()).collect::<Vec<String>>())
        }).unwrap_or_default();

        let mut inner_results = Map::new();
        for r_name in rollups {
            let res = validate_one_rollup(&r_name, base_view, t_start, t_end);
            inner_results.insert(r_name, res);
        }
        Value::Object(inner_results)
    } else {
        let res = validate_one_rollup(view_name, base_view, t_start, t_end);
        results.insert(view_name.to_string(), res);
        Value::Object(results)
    };
    crate::SKIP_ACCELERATION.with(|s| s.set(false));

    pgrx::JsonB(final_res)
}

fn validate_one_rollup(
    rollup_view: &str,
    base_view: &str,
    t_start: pgrx::datum::TimestampWithTimeZone,
    t_end: pgrx::datum::TimestampWithTimeZone,
) -> Value {
    let sources = Spi::connect(|client| {
        let sql = format!(
            "SELECT base_column, formula, mat_column FROM spiral.sources WHERE view_name = '{}'",
            rollup_view.replace("'", "''")
        );
        client.select(&sql, None, &[]).map(|r| {
            r.map(|row| {
                (
                    row.get::<String>(1).unwrap().unwrap(),
                    row.get::<String>(2).unwrap().unwrap(),
                    row.get::<String>(3).unwrap().unwrap(),
                )
            })
            .collect::<Vec<(String, String, String)>>()
        })
    })
    .unwrap_or_default();

    if sources.is_empty() {
        return json!({"status": "no_sources"});
    }

    let meta = catalog::get_metadata(rollup_view).unwrap();
    let scope_cols = meta.scope_columns;
    let scope_cols_str = if scope_cols.is_empty() {
        String::new()
    } else {
        scope_cols
            .iter()
            .map(|c| format!("\"{}\"", c))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut base_aggs = Vec::new();
    let mut rollup_aggs = Vec::new();
    let mut col_info = Vec::new();

    for (base_col, formula, mat_col) in sources {
        match formula.as_str() {
            "sum" | "count" => {
                base_aggs.push(format!("sum(\"{}\") as \"{}\"", base_col, mat_col));
                rollup_aggs.push(format!("sum(\"{}\") as \"{}\"", mat_col, mat_col));
                col_info.push((mat_col, "numeric".to_string()));
            }
            "stats" => {
                base_aggs.push(format!("spiral_stats(\"{}\") as \"{}\"", base_col, mat_col));
                rollup_aggs.push(format!(
                    "spiral_stats_merge(\"{}\") as \"{}\"",
                    mat_col, mat_col
                ));
                col_info.push((mat_col, "complex".to_string()));
            }
            "sketch" => {
                base_aggs.push(format!(
                    "spiral_sketch(\"{}\") as \"{}\"",
                    base_col, mat_col
                ));
                rollup_aggs.push(format!(
                    "spiral_sketch_merge(\"{}\") as \"{}\"",
                    mat_col, mat_col
                ));
                col_info.push((mat_col, "complex".to_string()));
            }
            "tdigest" => {
                base_aggs.push(format!(
                    "spiral_tdigest(\"{}\") as \"{}\"",
                    base_col, mat_col
                ));
                rollup_aggs.push(format!(
                    "spiral_tdigest_merge(\"{}\") as \"{}\"",
                    mat_col, mat_col
                ));
                col_info.push((mat_col, "complex".to_string()));
            }
            "ohlcv" => {
                base_aggs.push(format!("sum(\"{}\") as \"{}_v\"", base_col, mat_col));
                rollup_aggs.push(format!("sum(\"{}_v\") as \"{}_v\"", mat_col, mat_col));
                col_info.push((format!("{}_v", mat_col), "numeric".to_string()));
            }
            _ => {}
        }
    }

    if base_aggs.is_empty() {
        return json!({"status": "nothing_to_validate"});
    }

    let group_by = if scope_cols_str.is_empty() {
        String::new()
    } else {
        format!(" GROUP BY {}", scope_cols_str)
    };
    let select_scopes = if scope_cols_str.is_empty() {
        String::new()
    } else {
        format!("{}, ", scope_cols_str)
    };

    let base_sql = format!(
        "SELECT {} {} FROM \"{}\" WHERE t >= '{}' AND t < '{}' {}",
        select_scopes,
        base_aggs.join(", "),
        base_view.replace("\"", "\"\""),
        t_start.to_iso_string(),
        t_end.to_iso_string(),
        group_by
    );

    let rollup_sql = format!(
        "SELECT {} {} FROM \"{}\" WHERE t >= '{}' AND t < '{}' {}",
        select_scopes,
        rollup_aggs.join(", "),
        rollup_view.replace("\"", "\"\""),
        t_start.to_iso_string(),
        t_end.to_iso_string(),
        group_by
    );

    notice!("Spiral: validate_one_rollup base_sql: '{}'", base_sql);
    notice!("Spiral: validate_one_rollup rollup_sql: '{}'", rollup_sql);

    let mut mismatches = Vec::new();

    Spi::connect(|client| {
        let base_res = client.select(&base_sql, None, &[])?;
        let rollup_res = client.select(&rollup_sql, None, &[])?;

        notice!(
            "Spiral: validate_one_rollup base_res count: {}",
            base_res.len()
        );
        notice!(
            "Spiral: validate_one_rollup rollup_res count: {}",
            rollup_res.len()
        );

        let mut rollup_map = Map::new();
        for row in rollup_res {
            let key = if scope_cols.is_empty() {
                "TOTAL".to_string()
            } else {
                let mut keys = Vec::new();
                for i in 0..scope_cols.len() {
                    keys.push(
                        row.get::<String>(i + 1)
                            .unwrap()
                            .unwrap_or_else(|| "NULL".to_string()),
                    );
                }
                keys.join("|")
            };

            let mut vals = Map::new();
            for (i, (col, kind)) in col_info.iter().enumerate() {
                let offset = if scope_cols.is_empty() {
                    1
                } else {
                    scope_cols.len() + 1
                };
                if kind == "numeric" {
                    let v = row.get::<f64>(i + offset)?.unwrap_or(0.0);
                    vals.insert(col.clone(), json!(v));
                } else {
                    vals.insert(col.clone(), json!("COMPLEX"));
                }
            }
            rollup_map.insert(key, Value::Object(vals));
        }

        for row in base_res {
            let key = if scope_cols.is_empty() {
                "TOTAL".to_string()
            } else {
                let mut keys = Vec::new();
                for i in 0..scope_cols.len() {
                    keys.push(
                        row.get::<String>(i + 1)
                            .unwrap()
                            .unwrap_or_else(|| "NULL".to_string()),
                    );
                }
                keys.join("|")
            };

            let rollup_vals = match rollup_map.get(&key) {
                Some(Value::Object(m)) => m,
                _ => {
                    mismatches.push(json!({"key": key, "reason": "missing_in_rollup"}));
                    continue;
                }
            };

            for (i, (col, kind)) in col_info.iter().enumerate() {
                let offset = if scope_cols.is_empty() {
                    1
                } else {
                    scope_cols.len() + 1
                };
                if kind == "numeric" {
                    let base_v = row.get::<f64>(i + offset)?.unwrap_or(0.0);
                    let rollup_v = rollup_vals.get(col).and_then(|v| v.as_f64()).unwrap_or(0.0);

                    notice!(
                        "Spiral: validate_one_rollup comparing '{}': base={}, rollup={}",
                        col,
                        base_v,
                        rollup_v
                    );

                    if (base_v - rollup_v).abs() > 1e-6 {
                        mismatches.push(json!({
                            "key": key,
                            "column": col,
                            "base": base_v,
                            "rollup": rollup_v,
                            "diff": base_v - rollup_v
                        }));
                    }
                }
            }
        }

        Ok::<(), spi::Error>(())
    })
    .unwrap();

    if mismatches.is_empty() {
        json!({"status": "ok"})
    } else {
        json!({"status": "mismatch", "mismatches": mismatches})
    }
}

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    #[pg_test]
    fn test_spiral_validate_basic() {
        Spi::run("CREATE TABLE v_test (t timestamptz, val double precision)").unwrap();
        // Register base table metadata manually to enable spiral_refresh('v_test')
        Spi::run("INSERT INTO spiral.metadata (view_name, parent_view, frame_seconds, base_view, scope_columns) VALUES ('v_test', 'BASE', 0, 'v_test', '{}')").unwrap();

        Spi::run("SELECT spiral_register_view('v_test_1m', 'v_test', 60, 'v_test', '{}')").unwrap();
        Spi::run("INSERT INTO v_test (t, val) VALUES ('2026-05-19 10:00:05Z', 10.0), ('2026-05-19 10:00:55Z', 20.0)").unwrap();

        let changelog: pgrx::JsonB = Spi::get_one(
            "SELECT jsonb_agg(row_to_json(c)) FROM spiral.changelog c WHERE base_view = 'v_test'",
        )
        .unwrap()
        .unwrap();
        notice!("Spiral test: changelog content = {}", changelog.0);

        Spi::run("SELECT spiral_refresh('v_test')").unwrap();

        let res: pgrx::JsonB = Spi::get_one(
            "SELECT spiral_validate('v_test_1m', '2026-05-19 10:00:00Z', '2026-05-19 10:01:00Z')",
        )
        .unwrap()
        .unwrap();
        assert_eq!(res.0["v_test_1m"]["status"], "ok");

        // Corrupt it
        Spi::run("UPDATE v_test_1m SET val = 100.0").unwrap();
        let res2: pgrx::JsonB = Spi::get_one(
            "SELECT spiral_validate('v_test_1m', '2026-05-19 10:00:00Z', '2026-05-19 10:01:00Z')",
        )
        .unwrap()
        .unwrap();
        assert_eq!(res2.0["v_test_1m"]["status"], "mismatch");
        assert!(res2.0["v_test_1m"]["mismatches"].is_array());
    }
}
