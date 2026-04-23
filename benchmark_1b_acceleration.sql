-- Benchmark: Aspiral 1 Billion Row Transparent Acceleration
SET client_min_messages TO NOTICE;

-- Force fresh schema
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral CASCADE;
LOAD 'aspiral';

SET aspiral.kickoff_date = '2026-04-15';

-- 1. Setup
DROP TABLE IF EXISTS stress_raw CASCADE;

CREATE TABLE stress_raw (
    t timestamptz NOT NULL,
    val double precision
);

-- Register Aspiral Hierarchy
SELECT aspiral_register_view('stress_raw_ohlcv_1m', 'BASE', 60, 'stress_raw', ARRAY[]::text[]);
SELECT aspiral_register_view('stress_raw_ohlcv_1h', 'stress_raw_ohlcv_1m', 3600, 'stress_raw', ARRAY[]::text[]);
SELECT aspiral_register_view('stress_raw_ohlcv_1d', 'stress_raw_ohlcv_1h', 86400, 'stress_raw', ARRAY[]::text[]);

-- 2. Fast Ingestion (10 Million Rows)
CREATE OR REPLACE PROCEDURE ingest_demo_data(total_target BIGINT, batch_size INT)
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
        
        RAISE NOTICE 'Ingested % / % rows (%)', current_row, total_target, round((current_row::numeric / total_target::numeric) * 100, 2);
    END LOOP;
END;
$$;

\timing on
\echo '--- [STAGE 1] Ingesting 10 Million Rows ---'
CALL ingest_demo_data(10000000, 1000000);

-- 3. Materialize the Hierarchy
\echo '--- [STAGE 2] Materializing Rollup Hierarchy ---'
\echo 'Changelog state before refresh:'
SELECT * FROM aspiral.changelog;

SELECT aspiral_refresh('stress_raw_ohlcv_1m'); 

\echo 'Changelog state after refresh:'
SELECT * FROM aspiral.changelog;

-- 4. Benchmark performance
\echo '--- [STAGE 3] Query Acceleration Benchmark ---'

\echo '--- [ASPIRAL] Multi-Tiered Union Slicing EXPLAIN ---'
EXPLAIN (COSTS OFF) SELECT sum(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 01:30:05Z'::timestamptz;

\echo '--- [EXECUTION] Running Accelerated Query (1.5 Hours range) ---'
SELECT sum(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 01:30:05Z'::timestamptz;

\timing off
