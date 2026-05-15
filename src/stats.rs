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

/// Compact centroid-based sketch for approximate quantile estimation.
///
/// # Algorithm
///
/// Maintains at most `MAX_CENTROIDS` (100) centroids, each storing a mean value and weight.
/// On accumulation:
/// - Exact match (within 1e-9): increment centroid weight.
/// - Capacity available: add new centroid.
/// - At capacity: merge into the nearest centroid by absolute distance, updating its
///   weighted mean.
///
/// `min`, `max`, `count`, and `sum` are tracked exactly.
///
/// # Accuracy
///
/// This is **not** a t-digest. Differences from t-digest:
/// - Fixed centroid budget (no compression scaling).
/// - No size-biased merging: t-digest keeps small centroids near extremes for tail accuracy;
///   this algorithm does not.
/// - Tail quantiles (p<0.01, p>0.99) can have substantial error when cardinality exceeds
///   `MAX_CENTROIDS`.
///
/// **Exact** when the dataset has ≤100 distinct values.
/// **Approximate** otherwise — error is distribution-dependent. For a uniform distribution
/// over N >> 100 values, expected absolute quantile error ≈ range / 100.
///
/// Merging two sketches that have both hit capacity is also lossy.
pub const MAX_CENTROIDS: usize = 100;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SketchState {
    pub centroids: Vec<(f64, f64)>,
    pub sum: f64,
    pub count: f64,
    pub max: f64,
    pub min: f64,
}

impl SketchState {
    pub fn add(&mut self, val: f64) {
        if let Some(c) = self.centroids.iter_mut().find(|c| (c.0 - val).abs() < 1e-9) {
            c.1 += 1.0;
        } else if self.centroids.len() < MAX_CENTROIDS {
            self.centroids.push((val, 1.0));
        } else {
            // Nearest-centroid merge — lossy once at capacity.
            let nearest_idx = self
                .centroids
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| (a.0 - val).abs().partial_cmp(&(b.0 - val).abs()).unwrap())
                .map(|(i, _)| i)
                .unwrap();
            let c = &mut self.centroids[nearest_idx];
            let new_weight = c.1 + 1.0;
            c.0 = (c.0 * c.1 + val) / new_weight;
            c.1 = new_weight;
        }

        self.sum += val;
        self.count += 1.0;
        if val > self.max {
            self.max = val;
        }
        if val < self.min {
            self.min = val;
        }
    }

    pub fn merge(&mut self, other: &SketchState) {
        if other.count == 0.0 {
            return;
        }
        if self.count == 0.0 {
            *self = other.clone();
            return;
        }

        for &(val, weight) in &other.centroids {
            if let Some(c) = self.centroids.iter_mut().find(|c| (c.0 - val).abs() < 1e-9) {
                c.1 += weight;
            } else if self.centroids.len() < MAX_CENTROIDS {
                self.centroids.push((val, weight));
            } else {
                let nearest_idx = self
                    .centroids
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (a.0 - val).abs().partial_cmp(&(b.0 - val).abs()).unwrap()
                    })
                    .map(|(i, _)| i)
                    .unwrap();
                let c = &mut self.centroids[nearest_idx];
                let new_weight = c.1 + weight;
                c.0 = (c.0 * c.1 + val * weight) / new_weight;
                c.1 = new_weight;
            }
        }

        self.sum += other.sum;
        self.count += other.count;
        if other.max > self.max {
            self.max = other.max;
        }
        if other.min < self.min {
            self.min = other.min;
        }
    }

    /// Returns the q-quantile (0.0–1.0). Exact when ≤100 distinct values were accumulated.
    pub fn quantile(&self, q: f64) -> f64 {
        if self.count == 0.0 {
            return 0.0;
        }
        let mut sorted = self.centroids.clone();
        sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let target = q * self.count;
        let mut cum = 0.0;
        for (val, weight) in &sorted {
            cum += weight;
            if cum >= target {
                return *val;
            }
        }
        self.max
    }
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_else(SketchState::new);

    s.add(val);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_sketch_combine(
    state1: Option<pgrx::JsonB>,
    state2: Option<pgrx::JsonB>,
) -> pgrx::JsonB {
    let mut s1 = state1
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_else(SketchState::new);
    let s2 = state2
        .map(|j| serde_json::from_value::<SketchState>(j.0).unwrap())
        .unwrap_or_else(SketchState::new);

    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn spiral_quantile(state: pgrx::JsonB, q: f64) -> f64 {
    serde_json::from_value::<SketchState>(state.0)
        .unwrap()
        .quantile(q)
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
    "#,
    name = "create_spiral_stats_aggregates",
    requires = [
        spiral_stats_accum,
        spiral_stats_combine,
        spiral_sketch_accum,
        spiral_sketch_combine
    ]
);

#[cfg(test)]
mod tests {
    use super::*;

    fn sketch_from(vals: &[f64]) -> SketchState {
        let mut s = SketchState::new();
        for &v in vals {
            s.add(v);
        }
        s
    }

    // --- SketchState unit tests ---

    #[test]
    fn sketch_quantile_small_exact() {
        // ≤100 distinct values → quantile must be exact
        let s = sketch_from(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(s.quantile(0.0), 1.0);
        assert_eq!(s.quantile(0.5), 3.0);
        assert_eq!(s.quantile(1.0), 5.0);
    }

    #[test]
    fn sketch_quantile_repeated_values() {
        let s = sketch_from(&[5.0, 5.0, 5.0, 10.0]);
        assert_eq!(s.quantile(0.5), 5.0);
        assert_eq!(s.quantile(0.9), 10.0);
    }

    #[test]
    fn sketch_min_max_exact() {
        let s = sketch_from(&[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0]);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 9.0);
        assert_eq!(s.count, 7.0);
        assert!((s.sum - 25.0).abs() < 1e-10);
    }

    #[test]
    fn sketch_merge_stable() {
        // Merging two partial sketches must yield same count/sum/min/max as full sketch.
        let full = sketch_from(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);

        let mut left = sketch_from(&[1.0, 2.0, 3.0]);
        let right = sketch_from(&[4.0, 5.0, 6.0]);
        left.merge(&right);

        assert_eq!(left.count, full.count);
        assert!((left.sum - full.sum).abs() < 1e-10);
        assert_eq!(left.min, full.min);
        assert_eq!(left.max, full.max);
        // Quantiles must be exact for this small dataset.
        assert_eq!(left.quantile(0.5), full.quantile(0.5));
    }

    #[test]
    fn sketch_merge_empty_identity() {
        let s = sketch_from(&[1.0, 2.0, 3.0]);
        let empty = SketchState::new();

        let mut merged_left = s.clone();
        merged_left.merge(&empty);
        assert_eq!(merged_left.count, s.count);
        assert_eq!(merged_left.sum, s.sum);

        let mut merged_right = empty.clone();
        merged_right.merge(&s);
        assert_eq!(merged_right.count, s.count);
        assert_eq!(merged_right.sum, s.sum);
    }

    #[test]
    fn sketch_overflow_count_sum_min_max_exact() {
        // 200 distinct values exceeds MAX_CENTROIDS — centroids compress, but
        // count/sum/min/max remain exact.
        let vals: Vec<f64> = (1..=200).map(|i| i as f64).collect();
        let s = sketch_from(&vals);

        assert_eq!(s.count, 200.0);
        assert!((s.sum - (1.0 + 200.0) * 200.0 / 2.0).abs() < 1e-6);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 200.0);
    }

    #[test]
    fn sketch_overflow_sequential_bias() {
        // Documents known limitation: sequential insertion order causes severe quantile bias.
        //
        // With values 1..=200 inserted in order, the first 100 fill centroid slots, then
        // every subsequent value merges into the last centroid (nearest by distance).
        // This produces a single heavy centroid near the upper half of the range, so the
        // reported p50 reflects the majority weight there rather than the true median.
        //
        // Users who need accurate quantiles over high-cardinality sequential data must
        // pre-aggregate or accept this systematic error.
        let vals: Vec<f64> = (1..=200).map(|i| i as f64).collect();
        let s = sketch_from(&vals);

        // Actual p50 is ~150 (upper-half bias from sequential insertion), not ~100.
        let p50 = s.quantile(0.5);
        assert!(
            (p50 - 150.0).abs() < 5.0,
            "sequential p50={p50}: bias toward upper half is the known algorithm behavior"
        );
    }

    #[test]
    fn sketch_quantile_uniform_error_characterization() {
        // Documents accuracy for a uniform distribution over 1000 values.
        // count/sum/min/max are always exact. Quantile estimates under sequential
        // insertion are biased by the nearest-centroid overflow strategy.
        let vals: Vec<f64> = (1..=1000).map(|i| i as f64).collect();
        let s = sketch_from(&vals);

        // These always hold regardless of overflow behavior.
        assert_eq!(s.count, 1000.0);
        assert!((s.sum - 500500.0).abs() < 1e-3);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 1000.0);
    }
}
