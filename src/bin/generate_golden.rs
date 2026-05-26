use serde::Serialize;
use std::fs::File;
use std::io::Write;

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
    ohlcv_open: f64,
    ohlcv_high: f64,
    ohlcv_low: f64,
    ohlcv_close: f64,
    ohlcv_volume: f64,
    zorder_at_0: String,
    zorder_at_max_u32: String,
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

    // Calculate OHLCV (using first/last of the input sequence)
    let ohlcv_open = values[0];
    let ohlcv_high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let ohlcv_low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let ohlcv_close = *values.last().unwrap();
    let ohlcv_volume = values.iter().sum();

    // Z-Order reference (128-bit)
    // We can't easily call crate::zorder here without complex setup, 
    // so we'll implement a simple reference check for bit-stability.
    fn spread_64(x: u64) -> u128 {
        let mut res: u128 = x as u128;
        res = (res | (res << 32)) & 0x00000000FFFFFFFF00000000FFFFFFFF_u128;
        res = (res | (res << 16)) & 0x0000FFFF0000FFFF0000FFFF0000FFFF_u128;
        res = (res | (res << 8)) & 0x00FF00FF00FF00FF00FF00FF00FF00FF_u128;
        res = (res | (res << 4)) & 0x0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F0F_u128;
        res = (res | (res << 2)) & 0x33333333333333333333333333333333_u128;
        res = (res | (res << 1)) & 0x55555555555555555555555555555555_u128;
        res
    }
    let zorder_at_0 = (spread_64(0) | (spread_64(0) << 1)).to_string();
    let zorder_at_max_u32 = (spread_64(u32::MAX as u64) | (spread_64(0) << 1)).to_string();

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
        ohlcv_open,
        ohlcv_high,
        ohlcv_low,
        ohlcv_close,
        ohlcv_volume,
        zorder_at_0,
        zorder_at_max_u32,
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
