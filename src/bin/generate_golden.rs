use std::fs::File;
use std::io::Write;
use serde::Serialize;

#[derive(Serialize)]
struct GoldenResult {
    n: f64,
    mean: f64,
    variance: f64,
    stddev: f64,
    skewness: f64,
    kurtosis: f64,
    p50: f64,
    p95: f64,
    p99: f64,
}

fn main() {
    let mut values = Vec::new();
    
    // Set 1: Normal-ish distribution
    for i in 0..1000 {
        values.push(i as f64);
    }
    
    // Set 2: Outliers
    values.push(10000.0);
    values.push(-5000.0);
    
    // Calculate Golden Stats using stable two-pass
    let n = values.len() as f64;
    let mean: f64 = values.iter().sum::<f64>() / n;
    
    let mut m2 = 0.0;
    let mut m3 = 0.0;
    let mut m4 = 0.0;
    
    for &x in &values {
        let delta = x - mean;
        let delta2 = delta * delta;
        m2 += delta2;
        m3 += delta2 * delta;
        m4 += delta2 * delta2;
    }
    
    let variance = m2 / (n - 1.0);
    let stddev = variance.sqrt();
    
    // Fisher-Pearson definitions
    let skewness = (n.sqrt() * m3) / m2.powf(1.5);
    let kurtosis = (n * m4) / (m2 * m2) - 3.0;

    let mut sorted_values = values.clone();
    sorted_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    
    let get_quantile = |q: f64| {
        let idx = (q * (n - 1.0)).round() as usize;
        sorted_values[idx]
    };

    let result = GoldenResult {
        n,
        mean,
        variance,
        stddev,
        skewness,
        kurtosis,
        p50: get_quantile(0.5),
        p95: get_quantile(0.95),
        p99: get_quantile(0.99),
    };
    
    // Write CSV
    let mut csv_file = File::create("tests/golden/values.csv").unwrap();
    for v in &values {
        writeln!(csv_file, "{}", v).unwrap();
    }
    
    // Write JSON
    let json_file = File::create("tests/golden/expected.json").unwrap();
    serde_json::to_writer_pretty(json_file, &result).unwrap();
    
    println!("Golden Reference Dataset generated successfully.");
}
