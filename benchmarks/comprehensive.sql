-- Unified Spiral Benchmark: Comprehensive Feature Validation
-- This benchmark runs a scaled-down 10M row test to guarantee features work, 
-- do not crash, and correctly bypass regular queries (Zero Interference).

SET client_min_messages TO NOTICE;

-- 1. Setup Environment
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral CASCADE;
LOAD 'spiral';

SET spiral.kickoff_date = '2026-04-15';

-- ============================================================================
-- TEST 1: Zero Interference (Regular Tables)
-- ============================================================================
\echo '--- [TEST 1] Zero Interference (Regular Tables) ---'
DROP TABLE IF EXISTS regular_table CASCADE;
CREATE TABLE regular_table (id SERIAL, val INT);
INSERT INTO regular_table (val) SELECT random() * 1000 FROM generate_series(1, 10000);

-- Explain should show a standard Seq Scan without any Spiral notices or hook modifications
\echo '-> Validating non-spiral EXPLAIN:'
EXPLAIN (COSTS OFF) SELECT sum(val) FROM regular_table WHERE id > 5000;

-- ============================================================================
-- TEST 2: Core Time-Series Acceleration (10M Rows, No Scope)
-- ============================================================================
\echo '--- [TEST 2] Core Time-Series Acceleration (10M Rows) ---'
DROP TABLE IF EXISTS stress_raw CASCADE;
CREATE TABLE stress_raw (
    t timestamptz NOT NULL,
    val double precision
);

-- Register Spiral Hierarchy
SELECT spiral_register_view('stress_raw_1m', 'BASE', 60, 'stress_raw', ARRAY[]::text[]);
SELECT spiral_register_view('stress_raw_1h', 'stress_raw_1m', 3600, 'stress_raw', ARRAY[]::text[]);

-- Ingest 10M rows in batches
CREATE OR REPLACE PROCEDURE ingest_core_data(total_target BIGINT, batch_size INT)
LANGUAGE plpgsql AS $$
DECLARE
    current_row BIGINT := 0;
BEGIN
    WHILE current_row < total_target LOOP
        INSERT INTO stress_raw (t, val)
        SELECT 
            '2026-04-15 00:00:00Z'::timestamptz + (n || ' milliseconds')::interval,
            random() * 100
        FROM generate_series(current_row, LEAST(current_row + batch_size - 1, total_target - 1)) n;
        
        current_row := current_row + batch_size;
        COMMIT; 
    END LOOP;
END;
$$;

\timing on
\echo '-> Ingesting 100M Core Rows...'
CALL ingest_core_data(100000000, 1000000);
\timing off

\echo '-> Materializing Core Hierarchy...'
SELECT spiral_refresh('stress_raw'); 

\echo '-> EXPLAIN: 2-Hour Query (Should hit 1h, 1m, and Raw tiers)'
EXPLAIN (COSTS OFF) 
SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 02:30:05Z';

-- Make a dirty update
\echo '-> Inserting Dirty Row at 2026-04-15 01:15:00Z'
INSERT INTO stress_raw (t, val) VALUES ('2026-04-15 01:15:00Z', 9999);

\echo '-> EXPLAIN: 2-Hour Query with Dirty Fragment (Should fall back for 01:15:00)'
EXPLAIN (COSTS OFF) 
SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 02:30:05Z';


-- ============================================================================
-- TEST 3: IoT Multi-Tenant Scoped Acceleration
-- ============================================================================
\echo '--- [TEST 3] IoT Multi-Tenant Acceleration (Scope Columns) ---'
DROP TABLE IF EXISTS iot_raw CASCADE;
CREATE TABLE iot_raw (
    t timestamptz NOT NULL,
    device_id text NOT NULL,
    sensor_type text NOT NULL,
    reading double precision
);

-- Register Hierarchy with Scope
SELECT spiral_register_view('iot_raw_1m', 'BASE', 60, 'iot_raw', ARRAY['device_id', 'sensor_type']::text[]);
SELECT spiral_register_view('iot_raw_1h', 'iot_raw_1m', 3600, 'iot_raw', ARRAY['device_id', 'sensor_type']::text[]);

-- Ingest IoT Data
CREATE OR REPLACE PROCEDURE ingest_iot_data(total_target BIGINT)
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO iot_raw (t, device_id, sensor_type, reading)
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (n || ' seconds')::interval,
        'device_' || (n % 10)::text,
        'temp',
        random() * 100
    FROM generate_series(0, total_target - 1) n;
    COMMIT;
END;
$$;

\echo '-> Ingesting 1M IoT Rows (Multi-Tenant)...'
CALL ingest_iot_data(1000000);

\echo '-> Materializing IoT Hierarchy...'
SELECT spiral_refresh('iot_raw');

\echo '-> EXPLAIN: IoT Specific Device Query (Should pass scope_values down)'
EXPLAIN (COSTS OFF) 
SELECT sum(reading) FROM iot_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 05:00:00Z'
AND device_id = 'device_1' AND sensor_type = 'temp';

-- ============================================================================
-- TEST 4: Multi-Hierarchy Join Acceleration
-- ============================================================================
\echo '--- [TEST 4] Multi-Hierarchy Join Acceleration ---'
-- Accelerate BOTH tables by defining the range on only one side (Constraint Propagation)
\echo '-> EXPLAIN: Joining two independent Spiral hierarchies (Propagation Test)'
EXPLAIN (COSTS OFF)
SELECT sum(s.val + i.reading) 
FROM stress_raw s
JOIN iot_raw i ON s.t = i.t
WHERE s.t >= '2026-04-15 01:00:00Z' AND s.t < '2026-04-15 02:00:00Z';

\echo '-> EXECUTION: Running Join Acceleration...'
SELECT sum(s.val + i.reading) 
FROM stress_raw s
JOIN iot_raw i ON s.t = i.t
WHERE s.t >= '2026-04-15 01:00:00Z' AND s.t < '2026-04-15 02:00:00Z';

\echo '--- Comprehensive Benchmark Completed Successfully ---'
