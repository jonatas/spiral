-- Spiral Core Benchmark: Scaling, Accuracy, and Ingest Rate
-- Comparing Spiral against baseline PostgreSQL for time-series rollups and sketches.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS spiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS spiral_ticks CASCADE;
DROP TABLE IF EXISTS spiral_1m CASCADE;
DROP TABLE IF EXISTS spiral_5m CASCADE;
DROP TABLE IF EXISTS spiral_1h CASCADE;
DROP TYPE IF EXISTS time_value_pg CASCADE;

-- 2. SETUP
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2026-04-15';

-- Baseline Setup (PG-native aggregates)
CREATE TYPE time_value_pg AS (v double precision, t timestamptz);
CREATE OR REPLACE FUNCTION first_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts < state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE OR REPLACE FUNCTION last_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts >= state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE AGGREGATE first_pg(double precision, timestamptz) (sfunc = first_pg_sfunc, stype = time_value_pg);
CREATE AGGREGATE last_pg(double precision, timestamptz) (sfunc = last_pg_sfunc, stype = time_value_pg);

CREATE TABLE baseline_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);
CREATE TABLE spiral_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);

-- 3. INGESTION (Scaling Test)
DO $$
DECLARE
    rows int := 1000000; -- 1M rows for standard run
    start_time timestamptz;
    dur_base interval;
    dur_aspi interval;
BEGIN
    RAISE NOTICE '--- Starting 1M Row Ingestion (Baseline) ---';
    start_time := clock_timestamp();
    INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.01 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_base := clock_timestamp() - start_time;
    RAISE NOTICE 'Baseline Ingest: % (% rows/s)', dur_base, round(rows / extract(epoch from dur_base));

    RAISE NOTICE '--- Starting 1M Row Ingestion (Spiral) ---';
    start_time := clock_timestamp();
    INSERT INTO spiral_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.01 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_aspi := clock_timestamp() - start_time;
    RAISE NOTICE 'Spiral Ingest:  % (% rows/s)', dur_aspi, round(rows / extract(epoch from dur_aspi));
END $$;

-- 4. HIERARCHY CREATION
\echo '--- Creating Baseline Views ---'
CREATE MATERIALIZED VIEW baseline_1m AS SELECT date_trunc('minute', t) as t, symbol_id, (first_pg(price, t)).v as o, max(price) as h, min(price) as l, (last_pg(price, t)).v as c, sum(vol) as volume FROM baseline_ticks GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_1h AS SELECT date_trunc('hour', t) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_1m GROUP BY 1, 2;

\echo '--- Creating Spiral Views ---'
CREATE UNLOGGED TABLE spiral_1m AS 
SELECT to_timestamptz((spiral(t)/60)*60) as t, symbol_id,
    min(price) as o, max(price) as h, min(price) as l, max(price) as c,
    sum(vol) as volume, spiral_sketch(price) as price_sketch
FROM spiral_ticks GROUP BY 1, 2;

CREATE UNLOGGED TABLE spiral_1h AS 
SELECT to_timestamptz((spiral(t)/3600)*3600) as t, symbol_id,
    min(o) as o, max(h) as h, min(l) as l, max(c) as c,
    sum(volume) as volume, spiral_sketch_merge(price_sketch) as price_sketch
FROM spiral_1m GROUP BY 1, 2;

-- Inform Spiral about these views manually
SELECT spiral_register_view('spiral_1m', 'BASE', 60, 'spiral_ticks', ARRAY['symbol_id']);
SELECT spiral_register_view('spiral_1h', 'spiral_1m', 3600, 'spiral_ticks', ARRAY['symbol_id']);


-- 5. QUERY PERFORMANCE
\echo '--- P95 Percentile: Raw Data (1M Rows) ---'
EXPLAIN ANALYZE SELECT symbol_id, percentile_cont(0.95) WITHIN GROUP (ORDER BY price) FROM baseline_ticks WHERE symbol_id = 5 GROUP BY 1;

\echo '--- P95 Percentile: Spiral Sketch (Pre-aggregated 1h) ---'
EXPLAIN ANALYZE SELECT symbol_id, spiral_quantile(price_sketch, 0.95) FROM spiral_1h WHERE symbol_id = 5;

-- 6. STORAGE
SELECT relname as name, pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class WHERE relname IN ('baseline_ticks', 'spiral_ticks', 'spiral_1h', 'baseline_1h')
ORDER BY relname;
