use pgrx::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct StatsState {
    pub n: f64,
    pub m1: f64,
    pub m2: f64,
    pub m3: f64,
    pub m4: f64,
}

impl StatsState {
    /// Adds a new value to the statistics state using Welford's online algorithm.
    ///
    /// # Examples
    /// ```rust
    /// use spiral::stats::StatsState;
    ///
    /// let mut state = StatsState::default();
    /// state.add(10.0);
    /// state.add(20.0);
    /// state.add(30.0);
    ///
    /// assert_eq!(state.mean(), 20.0);
    /// assert_eq!(state.variance(), 100.0);
    /// assert_eq!(state.stddev(), 10.0);
    /// ```
    pub fn add(&mut self, x: f64) {
        let n1 = self.n;
        self.n += 1.0;
        let delta = x - self.m1;
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

    /// Merges another `StatsState` into this one, combining their statistics
    /// (Chan et al. parallel variance algorithm).
    ///
    /// # Examples
    /// ```rust
    /// use spiral::stats::StatsState;
    ///
    /// let mut s1 = StatsState::default();
    /// s1.add(10.0);
    /// s1.add(20.0);
    ///
    /// let mut s2 = StatsState::default();
    /// s2.add(30.0);
    /// s2.add(40.0);
    ///
    /// s1.merge(&s2);
    ///
    /// assert_eq!(s1.mean(), 25.0);
    /// assert!((s1.variance() - 166.66666666666666).abs() < 1e-9);
    /// ```
    pub fn merge(&mut self, other: &Self) {
        if other.n == 0.0 {
            return;
        }
        if self.n == 0.0 {
            *self = *other;
            return;
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

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state
        .map(|j| serde_json::from_value::<StatsState>(j.0).unwrap())
        .unwrap_or_default();
    s.add(val);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_combine(
    state1: Option<pgrx::JsonB>,
    state2: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    let mut s1 = state1
        .map(|j| serde_json::from_value::<StatsState>(j.0).unwrap())
        .unwrap_or_default();
    let s2 = state2
        .map(|j| serde_json::from_value::<StatsState>(j.0).unwrap())
        .unwrap_or_default();
    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_mean(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .unwrap()
        .mean()
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_variance(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .unwrap()
        .variance()
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_stddev(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .unwrap()
        .stddev()
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_skewness(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .unwrap()
        .skewness()
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_stats_kurtosis(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0)
        .unwrap()
        .kurtosis()
}

use tdigest::TDigest;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SketchState {
    pub td: TDigest,
    pub max: f64,
    pub min: f64,
}

impl Default for SketchState {
    fn default() -> Self {
        Self {
            td: TDigest::new_with_size(100),
            max: f64::MIN,
            min: f64::MAX,
        }
    }
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_default();

    s.td = s.td.merge_unsorted(vec![val]);
    if val > s.max {
        s.max = val;
    }
    if val < s.min {
        s.min = val;
    }

    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_combine(
    state1: Option<pgrx::JsonB>,
    state2: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    let mut s1 = state1
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_default();
    let s2 = state2
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_default();

    if s2.td.count() == 0.0 {
        return pgrx::JsonB(serde_json::to_value(s1).unwrap());
    }
    if s1.td.count() == 0.0 {
        return pgrx::JsonB(serde_json::to_value(s2).unwrap());
    }

    s1.td = TDigest::merge_digests(vec![s1.td, s2.td]);
    if s2.max > s1.max {
        s1.max = s2.max;
    }
    if s2.min < s1.min {
        s1.min = s2.min;
    }

    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_quantile(state: pgrx::JsonB, q: f64) -> f64 {
    let s = serde_json::from_value::<SketchState>(state.0).unwrap();
    if s.td.count() == 0.0 {
        return 0.0;
    }
    s.td.estimate_quantile(q)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_tdigest_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    spiral_sketch_accum(state, val)
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_tdigest_combine(
    state1: Option<pgrx::JsonB>,
    state2: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    spiral_sketch_combine(state1, state2)
}

extension_sql!(
    r#"
    CREATE OR REPLACE FUNCTION spiral_stats_mean(double precision) RETURNS double precision AS 'SELECT $1' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_stddev(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_variance(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_skewness(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION spiral_stats_kurtosis(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

    CREATE AGGREGATE spiral_stats(double precision) (
        SFUNC = spiral_stats_accum,
        STYPE = jsonb,
        COMBINEFUNC = spiral_stats_combine,
        PARALLEL = SAFE
    );

    CREATE AGGREGATE spiral_stats_merge(jsonb) (
        SFUNC = spiral_stats_combine,
        STYPE = jsonb,
        COMBINEFUNC = spiral_stats_combine,
        PARALLEL = SAFE
    );

    CREATE AGGREGATE spiral_sketch(double precision) (
        SFUNC = spiral_sketch_accum,
        STYPE = jsonb,
        COMBINEFUNC = spiral_sketch_combine,
        PARALLEL = SAFE
    );

    CREATE AGGREGATE spiral_sketch_merge(jsonb) (
        SFUNC = spiral_sketch_combine,
        STYPE = jsonb,
        COMBINEFUNC = spiral_sketch_combine,
        PARALLEL = SAFE
    );

    CREATE AGGREGATE spiral_tdigest(double precision) (
        SFUNC = spiral_tdigest_accum,
        STYPE = jsonb,
        COMBINEFUNC = spiral_tdigest_combine,
        PARALLEL = SAFE
    );

    CREATE AGGREGATE spiral_tdigest_merge(jsonb) (
        SFUNC = spiral_tdigest_combine,
        STYPE = jsonb,
        COMBINEFUNC = spiral_tdigest_combine,
        PARALLEL = SAFE
    );
    "#,
    name = "create_spiral_stats_aggregates",
    requires = [
        spiral_stats_accum,
        spiral_stats_combine,
        spiral_sketch_accum,
        spiral_sketch_combine,
        spiral_tdigest_accum,
        spiral_tdigest_combine
    ]
);
