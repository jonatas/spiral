-- Benchmark: Aspiral Transparent Query Acceleration
SET client_min_messages TO NOTICE;

-- Ensure the extension and library are loaded
CREATE EXTENSION IF NOT EXISTS aspiral CASCADE;
LOAD 'aspiral';

SET aspiral.kickoff_date = '2026-04-15';

-- 1. Setup
DROP TABLE IF EXISTS accel_bench CASCADE;
CREATE TABLE accel_bench (
    t timestamptz NOT NULL,
    val double precision
);

-- Register Aspiral Hierarchy manually (more reliable than WITH for benchmarks)
SELECT aspiral_register_view('accel_bench_ohlcv_1m', 'BASE', 60, 'accel_bench', ARRAY[]::text[]);
SELECT aspiral_register_view('accel_bench_ohlcv_1h', 'accel_bench_ohlcv_1m', 3600, 'accel_bench', ARRAY[]::text[]);

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
DROP TABLE IF EXISTS baseline_bench CASCADE;
CREATE TABLE baseline_bench AS SELECT * FROM accel_bench;

\timing on

-- Test 1: Full Range Sum (10 hours)
\echo '--- [BASELINE] Summing 10 hours of raw data ---'
SELECT sum(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 10:00:00Z'::timestamptz;

\echo '--- [ASPIRAL] Summing 10 hours (Transparent Acceleration) ---'
SELECT sum(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 10:00:00Z'::timestamptz;

-- Test 2: Partial Range (Mixed sources)
\echo '--- [BASELINE] Summing 5.5 hours of raw data ---'
SELECT sum(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 05:30:00Z'::timestamptz;

\echo '--- [ASPIRAL] Summing 5.5 hours (Mixed 1h + 1m acceleration) ---'
SELECT sum(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 05:30:00Z'::timestamptz;

-- Test 3: Average (Mathematical decomposition)
\echo '--- [BASELINE] AVG over 10 hours ---'
SELECT avg(val) FROM baseline_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 10:00:00Z'::timestamptz;

\echo '--- [ASPIRAL] AVG over 10 hours (Transparent decomposition) ---'
SELECT avg(val) FROM accel_bench WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 10:00:00Z'::timestamptz;

\timing off
