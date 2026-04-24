use pgrx::prelude::*;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct StatsState {
    pub n: f64,
    pub m1: f64,
    pub m2: f64,
    pub m3: f64,
    pub m4: f64,
}

impl StatsState {
    pub fn add(&mut self, x: f64) {
        let n1 = self.n;
        self.n += 1.0;
        let delta = x - self.m1;
        let delta_n = delta / self.n;
        let delta_n2 = delta_n * delta_n;
        let term1 = delta * delta_n * n1;

        self.m1 += delta_n;
        self.m4 += term1 * delta_n2 * (self.n * self.n - 3.0 * self.n + 3.0) + 6.0 * delta_n2 * self.m2 - 4.0 * delta_n * self.m3;
        self.m3 += term1 * delta_n * (self.n - 2.0) - 3.0 * delta_n * self.m2;
        self.m2 += term1;
    }

    pub fn merge(&mut self, other: &Self) {
        if other.n == 0.0 { return; }
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

        let m3 = self.m3 + other.m3 
            + delta3 * self.n * other.n * (self.n - other.n) / (combined_n * combined_n)
            + 3.0 * delta * (self.n * other.m2 - other.n * self.m2) / combined_n;

        let m4 = self.m4 + other.m4
            + delta4 * self.n * other.n * (self.n * self.n - self.n * other.n + other.n * other.n) / (combined_n * combined_n * combined_n)
            + 6.0 * delta2 * (self.n * self.n * other.m2 + other.n * other.n * self.m2) / (combined_n * combined_n)
            + 4.0 * delta * (self.n * other.m3 - other.n * self.m3) / combined_n;

        self.n = combined_n;
        self.m1 = m1;
        self.m2 = m2;
        self.m3 = m3;
        self.m4 = m4;
    }

    pub fn mean(&self) -> f64 { self.m1 }
    pub fn variance(&self) -> f64 { if self.n > 1.0 { self.m2 / (self.n - 1.0) } else { 0.0 } }
    pub fn stddev(&self) -> f64 { self.variance().sqrt() }
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
pub fn aspiral_stats_accum(state: Option<pgrx::JsonB>, val: f64) -> pgrx::JsonB {
    let mut s = state.map(|j| serde_json::from_value::<StatsState>(j.0).unwrap()).unwrap_or_default();
    s.add(val);
    pgrx::JsonB(serde_json::to_value(s).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_combine(state1: Option<pgrx::JsonB>, state2: Option<pgrx::JsonB>) -> pgrx::JsonB {
    let mut s1 = state1.map(|j| serde_json::from_value::<StatsState>(j.0).unwrap()).unwrap_or_default();
    let s2 = state2.map(|j| serde_json::from_value::<StatsState>(j.0).unwrap()).unwrap_or_default();
    s1.merge(&s2);
    pgrx::JsonB(serde_json::to_value(s1).unwrap())
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_mean(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0).unwrap().mean()
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_variance(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0).unwrap().variance()
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_stddev(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0).unwrap().stddev()
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_skewness(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0).unwrap().skewness()
}

#[pg_extern(immutable, parallel_safe)]
pub fn aspiral_stats_kurtosis(state: pgrx::JsonB) -> f64 {
    serde_json::from_value::<StatsState>(state.0).unwrap().kurtosis()
}

extension_sql!(
    r#"
    CREATE OR REPLACE FUNCTION aspiral_stats_mean(double precision) RETURNS double precision AS 'SELECT $1' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION aspiral_stats_stddev(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION aspiral_stats_variance(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION aspiral_stats_skewness(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;
    CREATE OR REPLACE FUNCTION aspiral_stats_kurtosis(double precision) RETURNS double precision AS 'SELECT 0.0::double precision' LANGUAGE SQL IMMUTABLE PARALLEL SAFE;

    CREATE AGGREGATE aspiral_stats(double precision) (
        SFUNC = aspiral_stats_accum,
        STYPE = jsonb,
        COMBINEFUNC = aspiral_stats_combine,
        PARALLEL = SAFE
    );
    
    CREATE AGGREGATE aspiral_stats_merge(jsonb) (
        SFUNC = aspiral_stats_combine,
        STYPE = jsonb,
        COMBINEFUNC = aspiral_stats_combine,
        PARALLEL = SAFE
    );
    "#,
    name = "create_aspiral_stats_aggregates",
    requires = [ aspiral_stats_accum, aspiral_stats_combine ]
);
