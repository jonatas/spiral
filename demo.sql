-- ============================================================================
-- ASPIRAL FULL COMPONENT PROOF & BENCHMARK
-- ============================================================================

-- 1. SETUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP SCHEMA IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral CASCADE;

SET aspiral.kickoff_date = '2026-04-15';

-- 2. TEMPORAL ANCHORING PROOF
SELECT 
    '2026-04-15 00:00:01Z'::timestamptz as original_time,
    aspiral('2026-04-15 00:00:01Z'::timestamptz) as offset_seconds,
    to_timestamptz(aspiral('2026-04-15 00:00:01Z'::timestamptz)) as roundtrip_time;

-- 3. ASPIRALING COORDINATE PROOF
SELECT 
    t as aspiral_time,
    to_aspiraling_number(t::bigint, 3600, 5) as coord_1h_cycle_lane_5
FROM (SELECT generate_series(0, 7200, 1800) as t) s;

-- 4. SPIRALING INDEX PROOF
DROP TABLE IF EXISTS seasonal_data;
CREATE TABLE seasonal_data (t timestamptz, val double precision);
CREATE INDEX idx_seasonal_spiral ON seasonal_data USING gist (to_spiral(aspiral(t), 86400));

INSERT INTO seasonal_data (t, val)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i || ' hours')::interval,
    random()
FROM generate_series(1, 1000) s(i);

EXPLAIN ANALYZE 
SELECT count(*) FROM seasonal_data 
WHERE to_spiral(aspiral(t), 86400) && '(-5000, 5000), (5000, 10000)'::box;

-- 5. MASSIVE O(1) BENCHMARK
DROP TABLE IF EXISTS benchmark_delta;
CREATE TABLE benchmark_delta (t bigint, tenant_id bigint, price double precision);
CREATE INDEX idx_delta_lookup ON benchmark_delta(t, tenant_id);

INSERT INTO benchmark_delta (t, tenant_id, price)
SELECT 
    (s.i / 1000) as t,
    (s.i % 1000) as tenant_id,
    (random() * 100)::double precision
FROM generate_series(0, 99999) s(i);

-- Physical packing
SELECT aspiral_pack_delta('benchmark_delta', 777);

\timing on
SELECT '--- [TEST 1] B-Tree Index Lookup ---' as benchmark;
SELECT * FROM benchmark_delta WHERE t = 50 AND tenant_id = 500;

SELECT '--- [TEST 2] Aspiraling O(1) Direct Read ---' as benchmark;
SELECT aspiral_read_main(777, 50, 500) as o1_value;
\timing off

-- ACCURACY
SELECT 
    (SELECT price FROM benchmark_delta WHERE t = 50 AND tenant_id = 500) as btree_val,
    (SELECT aspiral_read_main(777, 50, 500)) as o1_val,
    CASE 
        WHEN (SELECT price FROM benchmark_delta WHERE t = 50 AND tenant_id = 500) = (SELECT aspiral_read_main(777, 50, 500)) 
        THEN 'PASSED' ELSE 'FAILED' 
    END as status;

-- 6. PARTITIONING
DROP TABLE IF EXISTS partitioned_events;
CREATE TABLE partitioned_events (t bigint, event_name text) PARTITION BY RANGE (t);
SELECT aspiral_create_partition('partitioned_events', 86400, 0);
SELECT aspiral_create_partition('partitioned_events', 86400, 1);
\d+ partitioned_events
