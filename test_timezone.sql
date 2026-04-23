-- Aspiral Timezone-Aware Acceleration Test
SET client_min_messages TO NOTICE;
LOAD 'aspiral';

-- 1. Setup Data for a multi-day range
SET aspiral.kickoff_date = '2026-04-15';
DROP TABLE IF EXISTS tz_test CASCADE;
CREATE TABLE tz_test (t timestamptz NOT NULL, val double precision);

-- Register Hierarchy
SELECT aspiral_register_view('tz_test_1m', 'BASE', 60, 'tz_test', ARRAY[]::text[]);
SELECT aspiral_register_view('tz_test_1h', 'tz_test_1m', 3600, 'tz_test', ARRAY[]::text[]);
SELECT aspiral_register_view('tz_test_1d', 'tz_test_1h', 86400, 'tz_test', ARRAY[]::text[]);

-- Ingest 3 days of data
INSERT INTO tz_test (t, val)
SELECT '2026-04-15 00:00:00Z'::timestamptz + (n || ' minutes')::interval, 1.0
FROM generate_series(0, 60 * 24 * 3) n;

-- Materialize
SELECT aspiral_refresh('tz_test_1m');

-- ============================================================================
-- TEST 1: UTC Baseline (Perfect Alignment)
-- ============================================================================
\echo '--- [UTC] Perfect Alignment (Should hit Daily tier) ---'
SET TimeZone = 'UTC';
EXPLAIN (COSTS OFF) 
SELECT sum(val) FROM tz_test 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-17 00:00:00Z';

-- ============================================================================
-- TEST 2: Sao Paulo (Offset -3h)
-- ============================================================================
-- 2026-04-15 00:00:00 America/Sao_Paulo is 2026-04-15 03:00:00 UTC.
-- This shifted range should correctly use the Daily tier for the middle part
-- and fall back to Hourly/Minutely for the offset edges.
\echo '--- [America/Sao_Paulo] Shifted Alignment (-3h) ---'
SET TimeZone = 'America/Sao_Paulo';
EXPLAIN (COSTS OFF) 
SELECT sum(val) FROM tz_test 
WHERE t >= '2026-04-15 00:00:00'::timestamptz AND t < '2026-04-17 00:00:00'::timestamptz;

-- ============================================================================
-- TEST 3: New York (Offset -4h)
-- ============================================================================
\echo '--- [America/New_York] Shifted Alignment (-4h) ---'
SET TimeZone = 'America/New_York';
EXPLAIN (COSTS OFF) 
SELECT sum(val) FROM tz_test 
WHERE t >= '2026-04-15 00:00:00'::timestamptz AND t < '2026-04-17 00:00:00'::timestamptz;
