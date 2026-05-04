-- Spiral vs. PostgreSQL 100M Rows Stress Test (Optimized for Bulk Load)
-- Comparing Massive-Scale Storage, Aggregation, and Ingest Rate.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS spiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS spiral_ticks CASCADE;
DROP TYPE IF EXISTS time_value_pg CASCADE;

-- 2. SETUP
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2026-04-15';

-- 3. UNLOGGED TABLES FOR SPEED
CREATE UNLOGGED TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);

CREATE UNLOGGED TABLE spiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);

-- Aggregates
CREATE TYPE time_value_pg AS (v double precision, t timestamptz);
CREATE OR REPLACE FUNCTION first_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts < state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE OR REPLACE FUNCTION last_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts >= state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE AGGREGATE first_pg(double precision, timestamptz) (sfunc = first_pg_sfunc, stype = time_value_pg);
CREATE AGGREGATE last_pg(double precision, timestamptz) (sfunc = last_pg_sfunc, stype = time_value_pg);

-- 4. INGESTION (100M ROWS)
-- density: 1000 ticks/sec
DO $$
DECLARE
    rows int := 10000000; -- 10M (Adjust to 100M if hardware allows)
    start_time timestamptz;
    dur_base interval;
    dur_aspi interval;
BEGIN
    RAISE NOTICE '--- Starting 10M Row Ingestion (Baseline) ---';
    start_time := clock_timestamp();
    INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'),
        (i % 10),
        60000 + sin(i::float/1000)*1000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_base := clock_timestamp() - start_time;
    RAISE NOTICE 'Baseline 10M Ingest: % (% rows/s)', dur_base, round(rows / extract(epoch from dur_base));

    RAISE NOTICE '--- Starting 10M Row Ingestion (Spiral - Bulk) ---';
    start_time := clock_timestamp();
    INSERT INTO spiral_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'),
        (i % 10),
        60000 + sin(i::float/1000)*1000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_aspi := clock_timestamp() - start_time;
    RAISE NOTICE 'Spiral 10M Ingest:  % (% rows/s)', dur_aspi, round(rows / extract(epoch from dur_aspi));
END $$;

-- 5. INDEXING
\echo '--- Creating Indexes ---'
-- Optimized index for symbol-based filtering and grouping
CREATE INDEX idx_baseline_ticks_symbol_t ON baseline_ticks (symbol_id, t);
CREATE INDEX idx_spiral_ticks_symbol_t ON spiral_ticks (symbol_id, t);
-- Expression index to speed up the initial rollup aggregation
CREATE INDEX idx_spiral_ticks_spiral_t ON spiral_ticks (symbol_id, spiral(t));

-- 6. VIEW CREATION (Initial Aggregation)
\echo '--- Creating Baseline Views (Aggregating 10M rows) ---'
CREATE MATERIALIZED VIEW baseline_1m AS SELECT date_trunc('minute', t) as t, symbol_id, (first_pg(price, t)).v as o, max(price) as h, min(price) as l, (last_pg(price, t)).v as c, sum(vol) as volume FROM baseline_ticks GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_1h AS SELECT date_trunc('hour', t) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_1m GROUP BY 1, 2;
CREATE INDEX idx_baseline_1h_t_symbol ON baseline_1h (t, symbol_id);

\echo '--- Creating Spiral Hierarchy (Aggregating 10M rows + Sketches) ---'
-- This will automatically create spiral_5m and spiral_1h due to our fix in hooks.rs
CREATE MATERIALIZED VIEW spiral_1m WITH (spiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((spiral(t)/60)*60) as t, 
    symbol_id,
    first(price, spiral(t)) as o, max(price) as h, min(price) as l, last(price, spiral(t)) as c,
    sum(vol) as volume,
    spiral_sketch(price) as price_sketch
FROM spiral_ticks 
GROUP BY 1, 2;

CREATE INDEX idx_spiral_1h_t_symbol ON spiral_1h (t, symbol_id);

-- 7. PERFORMANCE QUERIES
\echo '--- P95 Percentile: Raw Data (10M Rows) ---'
EXPLAIN ANALYZE SELECT symbol_id, percentile_cont(0.95) WITHIN GROUP (ORDER BY price) FROM baseline_ticks WHERE symbol_id = 5 GROUP BY 1;

\echo '--- P95 Percentile: Spiral Sketch (Pre-aggregated 1h) ---'
EXPLAIN ANALYZE SELECT symbol_id, spiral_quantile(price_sketch, 0.95) FROM spiral_1h WHERE symbol_id = 5;

-- STORAGE
SELECT 
    relname as name,
    pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class 
WHERE relname IN ('baseline_ticks', 'spiral_ticks', 'spiral_1h', 'baseline_1h', 'spiral_1m', 'baseline_1m')
ORDER BY relname;
\n\n-- Acceleration Tests from 1B scenario\n
-- Benchmark: Spiral 1 Billion Row Transparent Acceleration
SET client_min_messages TO NOTICE;

-- Force fresh schema
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral CASCADE;
LOAD 'spiral';

SET spiral.kickoff_date = '2026-04-15';

-- 1. Setup
DROP TABLE IF EXISTS stress_raw CASCADE;

CREATE TABLE stress_raw (
    t timestamptz NOT NULL,
    val double precision
);

-- Register Spiral Hierarchy
SELECT spiral_register_view('stress_raw_1m', 'BASE', 60, 'stress_raw', ARRAY[]::text[]);
SELECT spiral_register_view('stress_raw_1h', 'stress_raw_1m', 3600, 'stress_raw', ARRAY[]::text[]);
SELECT spiral_register_view('stress_raw_1d', 'stress_raw_1h', 86400, 'stress_raw', ARRAY[]::text[]);

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
SELECT * FROM spiral.changelog;

SELECT spiral_refresh('stress_raw'); 

\echo 'Changelog state after refresh:'
SELECT * FROM spiral.changelog;

-- 4. Benchmark performance
\echo '--- [STAGE 3] Query Acceleration Benchmark ---'

\echo '--- [SPIRAL] Multi-Tiered Union Slicing EXPLAIN ---'
EXPLAIN (COSTS OFF) SELECT sum(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 01:30:05Z'::timestamptz;

\echo '--- [EXECUTION] Running Accelerated Query (1.5 Hours range) ---'
SELECT sum(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z'::timestamptz AND t < '2026-04-15 01:30:05Z'::timestamptz;

\timing off
