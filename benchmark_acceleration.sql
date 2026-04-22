-- Benchmark: Aspiral Transparent Query Acceleration
-- This script compares raw table aggregation vs. Aspiral's hierarchical cache

SET aspiral.kickoff_date = '2026-04-15';

-- 1. Setup
CREATE TABLE accel_bench (
    t timestamptz NOT NULL,
    val double precision -- Aspiral: sum, count, avg
) WITH (aspiral.frames = '1m,1h');

-- 2. Ingest 100,000 rows over 10 hours
INSERT INTO accel_bench (t, val)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (n || ' seconds')::interval,
    random() * 100
FROM generate_series(0, 36000) n;

-- 3. Materialize Aspiral Hierarchy
SELECT aspiral_refresh('accel_bench_ohlcv_1m');
SELECT aspiral_refresh('accel_bench_ohlcv_1h');

-- 4. Baseline Query (Non-accelerated)
-- We can disable the hook or just query a baseline table
CREATE TABLE baseline_bench AS SELECT * FROM accel_bench;

\timing on

-- Test 1: Full Range Sum (10 hours)
-- Expected: Raw scans 36,000 rows. Aspiral scans 10 rows from _1h view.
\echo '--- [BASELINE] Summing 10 hours of raw data ---'
SELECT sum(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 10:00:00Z';

\echo '--- [ASPIRAL] Summing 10 hours (Transparent Acceleration) ---'
SELECT sum(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 10:00:00Z';

-- Test 2: Partial Range (Mixed sources)
-- Query 5 hours and 30 minutes
\echo '--- [BASELINE] Summing 5.5 hours of raw data ---'
SELECT sum(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 05:30:00Z';

\echo '--- [ASPIRAL] Summing 5.5 hours (Mixed 1h + 1m acceleration) ---'
SELECT sum(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 05:30:00Z';

-- Test 3: Average (Mathematical decomposition)
\echo '--- [BASELINE] AVG over 10 hours ---'
SELECT avg(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 10:00:00Z';

\echo '--- [ASPIRAL] AVG over 10 hours (Transparent decomposition) ---'
SELECT avg(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 10:00:00Z';

\timing off

-- Cleanup
DROP TABLE accel_bench CASCADE;
DROP TABLE baseline_bench;
