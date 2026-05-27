use pgrx::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct StatsState {
    pub n: f64,
    pub m1: f64,
    pub m2: f64,
    pub m3: f64,
    pub m4: f64,
    pub min: f64,
    pub max: f64,
}

impl Default for StatsState {
    fn default() -> Self {
        StatsState {
            n: 0.0,
            m1: 0.0,
            m2: 0.0,
            m3: 0.0,
            m4: 0.0,
            min: f64::MAX,
            max: f64::MIN,
        }
    }
}

impl StatsState {
    pub fn add(&mut self, val: f64) {
        if val < self.min {
            self.min = val;
        }
        if val > self.max {
            self.max = val;
        }
        let n1 = self.n;
        self.n += 1.0;
        let delta = val - self.m1;
        let delta_n = delta / self.n;
        let delta_n2 = delta_n * delta_n;
        let term1 = delta * delta_n * n1;
        self.m1 += delta_n;
        self.m4 += term1 * delta_n2 * (self.n * self.n - 3.0 * self.n + 3.0)
            + 6.0 * delta_n2 * self.m2
            - 4.0 * delta_n * self.m3;
        self.m3 += term1 * delta_n * (self.n - 2.0) - 3.0 * delta_n * self.m2;
        self.m2 += term1;
    }

    pub fn merge(&mut self, other: &Self) {
        if other.n == 0.0 {
            return;
        }
        if self.n == 0.0 {
            *self = *other;
            return;
        }
        if other.min < self.min {
            self.min = other.min;
        }
        if other.max > self.max {
            self.max = other.max;
        }
        let combined_n = self.n + other.n;
        let delta = other.m1 - self.m1;
        let delta2 = delta * delta;
        let delta3 = delta2 * delta;
        let delta4 = delta3 * delta;
        let m1 = (self.n * self.m1 + other.n * other.m1) / combined_n;
        let m2 = self.m2 + other.m2 + delta2 * self.n * other.n / combined_n;
        let m3 = self.m3
            + other.m3
            + delta3 * self.n * other.n * (self.n - other.n) / (combined_n * combined_n)
            + 3.0 * delta * (self.n * other.m2 - other.n * self.m2) / combined_n;
        let m4 = self.m4
            + other.m4
            + delta4 * self.n * other.n * (self.n * self.n - self.n * other.n + other.n * other.n)
                / (combined_n * combined_n * combined_n)
            + 6.0 * delta2 * (self.n * self.n * other.m2 + other.n * other.n * self.m2)
                / (combined_n * combined_n)
            + 4.0 * delta * (self.n * other.m3 - other.n * self.m3) / combined_n;
        self.n = combined_n;
        self.m1 = m1;
        self.m2 = m2;
        self.m3 = m3;
        self.m4 = m4;
    }

    pub fn mean(&self) -> f64 {
        self.m1
    }
    pub fn variance(&self) -> f64 {
        if self.n > 1.0 {
            self.m2 / (self.n - 1.0)
        } else {
            0.0
        }
    }
    pub fn stddev(&self) -> f64 {
        self.variance().sqrt()
    }
    pub fn skewness(&self) -> f64 {
        if self.m2 > 0.0 {
            (self.n.sqrt() * self.m3) / self.m2.powf(1.5)
        } else {
            0.0
        }
    }
    pub fn kurtosis(&self) -> f64 {
        if self.m2 > 0.0 {
            (self.n * self.m4) / (self.m2 * self.m2) - 3.0
        } else {
            0.0
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SketchState {
    pub centroids: Vec<(f64, f64)>,
    pub count: f64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl Default for SketchState {
    fn default() -> Self {
        SketchState {
            centroids: Vec::new(),
            count: 0.0,
            sum: 0.0,
            min: f64::MAX,
            max: f64::MIN,
        }
    }
}

impl SketchState {
    pub fn add(&mut self, val: f64) {
        self.count += 1.0;
        self.sum += val;
        if val < self.min {
            self.min = val;
        }
        if val > self.max {
            self.max = val;
        }
        let mut found = false;
        for c in &mut self.centroids {
            if (c.0 - val).abs() < 1e-9 {
                c.1 += 1.0;
                found = true;
                break;
            }
        }
        if !found {
            self.centroids.push((val, 1.0));
        }
        if self.centroids.len() > 200 {
            self.compress();
        }
    }
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0.0 {
            return;
        }
        if self.count == 0.0 {
            *self = other.clone();
            return;
        }
        self.count += other.count;
        self.sum += other.sum;
        if other.min < self.min {
            self.min = other.min;
        }
        if other.max > self.max {
            self.max = other.max;
        }
        for c2 in &other.centroids {
            let mut found = false;
            for c1 in &mut self.centroids {
                if (c1.0 - c2.0).abs() < 1e-9 {
                    c1.1 += c2.1;
                    found = true;
                    break;
                }
            }
            if !found {
                self.centroids.push(*c2);
            }
        }
        if self.centroids.len() > 200 {
            self.compress();
        }
    }
    fn compress(&mut self) {
        self.centroids
            .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let mut new_centroids = Vec::new();
        if self.centroids.is_empty() {
            return;
        }
        let mut current = self.centroids[0];
        for next in self.centroids.iter().skip(1) {
            if new_centroids.len() < 100 {
                if (current.0 - next.0).abs() < (self.max - self.min).abs() / 100.0 + 1e-9 {
                    let combined_w = current.1 + next.1;
                    current.0 = (current.0 * current.1 + next.0 * next.1) / combined_w;
                    current.1 = combined_w;
                } else {
                    new_centroids.push(current);
                    current = *next;
                }
            } else {
                let combined_w = current.1 + next.1;
                current.0 = (current.0 * current.1 + next.0 * next.1) / combined_w;
                current.1 = combined_w;
            }
        }
        new_centroids.push(current);
        self.centroids = new_centroids;
    }
    pub fn quantile(&self, q: f64) -> f64 {
        if self.centroids.is_empty() {
            return 0.0;
        }
        if q <= 0.0 {
            return self.min;
        }
        if q >= 1.0 {
            return self.max;
        }
        let mut sorted = self.centroids.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let target_count = q * self.count;
        let mut current_count = 0.0;
        for i in 0..sorted.len() {
            let next_count = current_count + sorted[i].1;
            if next_count >= target_count {
                return sorted[i].0;
            }
            current_count = next_count;
        }
        self.max
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct OHLCVState {
    pub open: f64,
    pub open_t: i64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub close_t: i64,
    pub volume: f64,
    pub count: f64,
}

impl Default for OHLCVState {
    fn default() -> Self {
        OHLCVState {
            open: 0.0,
            open_t: i64::MAX,
            high: f64::MIN,
            low: f64::MAX,
            close: 0.0,
            close_t: i64::MIN,
            volume: 0.0,
            count: 0.0,
        }
    }
}

impl OHLCVState {
    pub fn add(&mut self, val: f64, t: i64) {
        if t < self.open_t {
            self.open = val;
            self.open_t = t;
        }
        if t >= self.close_t {
            self.close = val;
            self.close_t = t;
        }
        if val > self.high {
            self.high = val;
        }
        if val < self.low {
            self.low = val;
        }
        self.volume += val;
        self.count += 1.0;
    }
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0.0 {
            return;
        }
        if self.count == 0.0 {
            *self = *other;
            return;
        }
        if other.open_t < self.open_t {
            self.open = other.open;
            self.open_t = other.open_t;
        }
        if other.close_t >= self.close_t {
            self.close = other.close;
            self.close_t = other.close_t;
        }
        if other.high > self.high {
            self.high = other.high;
        }
        if other.low < self.low {
            self.low = other.low;
        }
        self.volume += other.volume;
        self.count += other.count;
    }
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_from_count(count: f64) -> pgrx::JsonB {
    let mut s = StatsState::default();
    s.n = count;
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state
        .map(|j| serde_json::from_value::<StatsState>(j.0).unwrap())
        .unwrap_or_default();
    s.add(val);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_combine(state: Option<pgrx::JsonB>, other: Option<pgrx::JsonB>) -> pgrx::JsonB {
    let mut s1 = state
        .and_then(|j| serde_json::from_value::<StatsState>(j.0).ok())
        .unwrap_or_default();
    let s2 = other
        .and_then(|j| serde_json::from_value::<StatsState>(j.0).ok())
        .unwrap_or_default();
    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_mean(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .mean()
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_count_final(state: pgrx::JsonB) -> i64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .n as i64
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_sum_final(state: pgrx::JsonB) -> f64 {
    let s = serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default();
    s.m1 * s.n
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_min_final(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .min
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_max_final(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .max
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_variance(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .variance()
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_stddev(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .stddev()
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_skewness(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .skewness()
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_kurtosis(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .ok()
        .unwrap_or_default()
        .kurtosis()
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state
        .and_then(|j| serde_json::from_value::<SketchState>(j.0).ok())
        .unwrap_or_default();
    s.add(val);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_combine(
    state: Option<pgrx::JsonB>,
    other: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    let mut s1 = state
        .and_then(|j| serde_json::from_value::<SketchState>(j.0).ok())
        .unwrap_or_default();
    let s2 = other
        .and_then(|j| serde_json::from_value::<SketchState>(j.0).ok())
        .unwrap_or_default();
    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_quantile(state: pgrx::JsonB, q: f64) -> f64 {
    serde_json::from_value::<SketchState>(state.0)
        .ok()
        .unwrap_or_default()
        .quantile(q)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_tdigest_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    spiral_sketch_accum(state, val)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_tdigest_combine(
    state: Option<pgrx::JsonB>,
    other: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    spiral_sketch_combine(state, other)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_accum(state: Option<pgrx::JsonB>, val: f64, t: i64) -> pgrx::JsonB {
    let mut s = state
        .and_then(|j| serde_json::from_value::<OHLCVState>(j.0).ok())
        .unwrap_or_default();
    s.add(val, t);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_combine(state: Option<pgrx::JsonB>, other: Option<pgrx::JsonB>) -> pgrx::JsonB {
    let mut s1 = state
        .and_then(|j| serde_json::from_value::<OHLCVState>(j.0).ok())
        .unwrap_or_default();
    let s2 = other
        .and_then(|j| serde_json::from_value::<OHLCVState>(j.0).ok())
        .unwrap_or_default();
    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_open(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default()
        .open
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_high(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default()
        .high
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_low(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default()
        .low
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_close(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default()
        .close
}
#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_volume(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default()
        .volume
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_to_array(state: pgrx::JsonB) -> Vec<f64> {
    let s = serde_json::from_value::<OHLCVState>(state.0)
        .ok()
        .unwrap_or_default();
    vec![s.open, s.high, s.low, s.close, s.volume]
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_ohlcv_to_json(state: pgrx::JsonB) -> pgrx::JsonB {
    state
}

extension_sql!(
    r#"
    CREATE OR REPLACE FUNCTION spiral_stats_mean(double precision) RETURNS double precision AS 'SELECT $1' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_stddev(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_variance(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_skewness(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_kurtosis(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

    CREATE AGGREGATE spiral_stats(double precision) (SFUNC = spiral_stats_accum, STYPE = jsonb, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_stats_merge(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_sketch(double precision) (SFUNC = spiral_sketch_accum, STYPE = jsonb, COMBINEFUNC = spiral_sketch_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_sketch_merge(jsonb) (SFUNC = spiral_sketch_combine, STYPE = jsonb, COMBINEFUNC = spiral_sketch_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_tdigest(double precision) (SFUNC = spiral_tdigest_accum, STYPE = jsonb, COMBINEFUNC = spiral_tdigest_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_tdigest_merge(jsonb) (SFUNC = spiral_tdigest_combine, STYPE = jsonb, COMBINEFUNC = spiral_tdigest_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_avg(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, FINALFUNC = spiral_stats_mean, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_count(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, FINALFUNC = spiral_stats_count_final, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_sum(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, FINALFUNC = spiral_stats_sum_final, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_min(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, FINALFUNC = spiral_stats_min_final, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_max(jsonb) (SFUNC = spiral_stats_combine, STYPE = jsonb, FINALFUNC = spiral_stats_max_final, COMBINEFUNC = spiral_stats_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_ohlcv(double precision, bigint) (SFUNC = spiral_ohlcv_accum, STYPE = jsonb, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_ohlcv_merge(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_open(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, FINALFUNC = spiral_ohlcv_open, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_high(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, FINALFUNC = spiral_ohlcv_high, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_low(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, FINALFUNC = spiral_ohlcv_low, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_close(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, FINALFUNC = spiral_ohlcv_close, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    CREATE AGGREGATE spiral_volume(jsonb) (SFUNC = spiral_ohlcv_combine, STYPE = jsonb, FINALFUNC = spiral_ohlcv_volume, COMBINEFUNC = spiral_ohlcv_combine, PARALLEL = SAFE);
    "#,
    name = "create_spiral_stats_aggregates",
    requires = [
        spiral_stats_accum,
        spiral_stats_combine,
        spiral_stats_from_count,
        spiral_stats_mean,
        spiral_stats_count_final,
        spiral_stats_sum_final,
        spiral_stats_min_final,
        spiral_stats_max_final,
        spiral_sketch_accum,
        spiral_sketch_combine,
        spiral_tdigest_accum,
        spiral_tdigest_combine,
        spiral_ohlcv_accum,
        spiral_ohlcv_combine,
        spiral_ohlcv_to_array,
        spiral_ohlcv_to_json
    ]
);
