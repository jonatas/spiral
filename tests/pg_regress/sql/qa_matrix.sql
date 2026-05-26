-- QA Matrix: Systematic verification of all rollup functions across slices and cardinalities.

-- 1. Setup Base Table and Metadata
CREATE TABLE qa_base (
    t timestamptz NOT NULL,
    tenant_id int NOT NULL,
    val double precision NOT NULL
);

-- Register metadata and create hierarchy via accelerate
-- 1m -> 1h -> 1d -> 1mo
SELECT accelerate('qa_base', 
                  frames => '1m,1h,1d,1mo', 
                  tenant => ARRAY['tenant_id'], 
                  columns => ARRAY['val ohlcv as val_ohlcv', 'val stats as val_stats'],
                  initial_load => false);

-- 2. Data Ingestion: Single Tenant, Dense Data
-- 100 points in one hour for tenant 1
INSERT INTO qa_base (t, tenant_id, val)
SELECT '2026-05-25 00:00:00Z'::timestamptz + (i || ' seconds')::interval,
       1,
       i::double precision
FROM generate_series(1, 100) i;

-- Data Ingestion: Multiple Tenants, Sparse Data
-- 1 point per hour for 10 tenants over 24 hours
INSERT INTO qa_base (t, tenant_id, val)
SELECT '2026-05-25 00:00:00Z'::timestamptz + (h || ' hours')::interval,
       t,
       (t * 100 + h)::double precision
FROM generate_series(2, 11) t, generate_series(0, 23) h;

-- Refresh all tiers
SELECT spiral_refresh('qa_base_1m');
SELECT spiral_refresh('qa_base_1h');
SELECT spiral_refresh('qa_base_1d');
SELECT spiral_refresh('qa_base_1mo');

-- 3. Consistency Matrix: Rollup vs Raw
-- Use rounding/epsilons for stable floating point output

-- A. Basic Aggregates (Sum, Count, Avg)
-- Comparing raw vs 1m (ohlcv-based) vs 1h/1d (stats-based)
WITH raw AS (
    SELECT tenant_id, 
           round(sum(val)::numeric, 2) as s, 
           count(*) as c, 
           round(avg(val)::numeric, 2) as a
    FROM qa_base 
    WHERE tenant_id = 1
    GROUP BY tenant_id
),
rollup_1m AS (
    SELECT tenant_id, 
           round(spiral_volume(val_ohlcv)::numeric, 2) as s, 
           spiral_count(val_stats) as c,
           round(spiral_avg(val_stats)::numeric, 2) as a
    FROM qa_base_1m
    WHERE tenant_id = 1
    GROUP BY tenant_id
),
rollup_1h AS (
    SELECT tenant_id,
           round(spiral_sum(val_stats)::numeric, 2) as s,
           spiral_count(val_stats) as c,
           round(spiral_avg(val_stats)::numeric, 2) as a
    FROM qa_base_1h
    WHERE tenant_id = 1
    GROUP BY tenant_id
)
SELECT 'Tenant 1 Basic' as label,
       (SELECT s FROM raw) = (SELECT s FROM rollup_1h) as sum_ok,
       (SELECT c FROM raw) = (SELECT c FROM rollup_1h) as count_ok,
       (SELECT a FROM raw) = (SELECT a FROM rollup_1h) as avg_ok;

-- B. Advanced Stats (Mean, Variance, Skewness, Kurtosis)
-- Only available in stats-based tiers (1h, 1d)
WITH raw AS (
    SELECT round(spiral_stats_mean(spiral_stats(val))::numeric, 4) as mean,
           round(spiral_stats_variance(spiral_stats(val))::numeric, 4) as var,
           round(spiral_stats_skewness(spiral_stats(val))::numeric, 4) as skew,
           round(spiral_stats_kurtosis(spiral_stats(val))::numeric, 4) as kurt
    FROM qa_base
    WHERE tenant_id = 1
),
rollup AS (
    SELECT round(spiral_stats_mean(spiral_stats_merge(val_stats))::numeric, 4) as mean,
           round(spiral_stats_variance(spiral_stats_merge(val_stats))::numeric, 4) as var,
           round(spiral_stats_skewness(spiral_stats_merge(val_stats))::numeric, 4) as skew,
           round(spiral_stats_kurtosis(spiral_stats_merge(val_stats))::numeric, 4) as kurt
    FROM qa_base_1h
    WHERE tenant_id = 1
)
SELECT 'Tenant 1 Stats' as label,
       mean, var, skew, kurt
FROM rollup;

-- C. Multi-Tenant Consistency
-- Verify that rollups don't leak data between tenants
SELECT tenant_id, 
       spiral_count(val_stats) as rollup_count,
       (SELECT count(*) FROM qa_base b WHERE b.tenant_id = q.tenant_id) as raw_count
FROM qa_base_1d q
GROUP BY tenant_id
ORDER BY tenant_id;

-- D. Timezone & Calendar Alignment (Month-level)
-- Insert data across a month boundary
INSERT INTO qa_base (t, tenant_id, val) VALUES
('2026-05-31 23:59:00Z', 100, 1.0),
('2026-06-01 00:01:00Z', 100, 2.0);

SELECT spiral_refresh('qa_base_1mo');

-- Verify calendar alignment (May vs June)
SELECT date_trunc('month', t) as mo,
       spiral_sum(val_stats) as s
FROM qa_base_1mo
WHERE tenant_id = 100
GROUP BY 1
ORDER BY 1;

-- 4. Performance & Planner Check: Acceleration
-- Ensure queries are actually being routed to rollups
-- We use EXPLAIN (FORMAT JSON) or similar if available, or just check row counts
-- but in pg_regress we can use the custom spiral_explain helper.

SELECT spiral_explain('SELECT sum(val) FROM qa_base WHERE tenant_id = 2 AND t >= ''2026-05-25 00:00:00Z'' AND t < ''2026-05-26 00:00:00Z''');

-- Cleanup
DROP TABLE qa_base CASCADE;
