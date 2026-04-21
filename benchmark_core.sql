-- Aspiral Core Benchmark: Scaling, Accuracy, and Ingest Rate
-- Comparing Aspiral against baseline PostgreSQL for time-series rollups and sketches.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_1m CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_5m CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_1h CASCADE;
DROP TYPE IF EXISTS time_value_pg CASCADE;

-- 2. SETUP
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

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
CREATE TABLE aspiral_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);

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

    RAISE NOTICE '--- Starting 1M Row Ingestion (Aspiral) ---';
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.01 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_aspi := clock_timestamp() - start_time;
    RAISE NOTICE 'Aspiral Ingest:  % (% rows/s)', dur_aspi, round(rows / extract(epoch from dur_aspi));
END $$;

-- 4. HIERARCHY CREATION
\echo '--- Creating Baseline Views ---'
CREATE MATERIALIZED VIEW baseline_ohlcv_1m AS SELECT date_trunc('minute', t) as t, symbol_id, (first_pg(price, t)).v as o, max(price) as h, min(price) as l, (last_pg(price, t)).v as c, sum(vol) as volume FROM baseline_ticks GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_ohlcv_1h AS SELECT date_trunc('hour', t) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_ohlcv_1m GROUP BY 1, 2;

\echo '--- Creating Aspiral Views ---'
CREATE MATERIALIZED VIEW aspiral_ohlcv_1m WITH (aspiral.frames='1h') AS 
SELECT to_timestamptz((aspiral(t)/60)*60) as t, symbol_id,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume, aspiral_sketch(price) as price_sketch
FROM aspiral_ticks GROUP BY 1, 2;

-- 5. QUERY PERFORMANCE
\echo '--- P95 Percentile: Raw Data (1M Rows) ---'
EXPLAIN ANALYZE SELECT symbol_id, percentile_cont(0.95) WITHIN GROUP (ORDER BY price) FROM baseline_ticks WHERE symbol_id = 5 GROUP BY 1;

\echo '--- P95 Percentile: Aspiral Sketch (Pre-aggregated 1h) ---'
EXPLAIN ANALYZE SELECT symbol_id, aspiral_quantile(price_sketch, 0.95) FROM aspiral_ohlcv_1h WHERE symbol_id = 5;

-- 6. STORAGE
SELECT relname as name, pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class WHERE relname IN ('baseline_ticks', 'aspiral_ticks', 'aspiral_ohlcv_1h', 'baseline_ohlcv_1h')
ORDER BY relname;
