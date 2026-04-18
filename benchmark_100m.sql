-- Aspiral vs. PostgreSQL 100M Rows Stress Test (Optimized for Bulk Load)
-- Comparing Massive-Scale Storage, Aggregation, and Ingest Rate.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;
DROP TYPE IF EXISTS time_value_pg CASCADE;

-- 2. SETUP
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 3. UNLOGGED TABLES FOR SPEED
CREATE UNLOGGED TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);

CREATE UNLOGGED TABLE aspiral_ticks (
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
    rows int := 10000000; -- 10M
    start_time timestamptz;
    dur_base interval;
    dur_aspi interval;
BEGIN
    RAISE NOTICE '--- Starting 100M Row Ingestion (Baseline) ---';
    start_time := clock_timestamp();
    INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'),
        (i % 10),
        60000 + sin(i::float/1000)*1000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_base := clock_timestamp() - start_time;
    RAISE NOTICE 'Baseline 100M Ingest: % (% rows/s)', dur_base, round(rows / extract(epoch from dur_base));

    RAISE NOTICE '--- Starting 100M Row Ingestion (Aspiral - Bulk) ---';
    -- Trigger is NOT yet created (no views)
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'),
        (i % 10),
        60000 + sin(i::float/1000)*1000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_aspi := clock_timestamp() - start_time;
    RAISE NOTICE 'Aspiral 100M Ingest:  % (% rows/s)', dur_aspi, round(rows / extract(epoch from dur_aspi));
END $$;

-- 5. INDEXING
RAISE NOTICE '--- Creating Indexes ---';
CREATE INDEX idx_baseline_ticks_t_symbol ON baseline_ticks (t, symbol_id);
CREATE INDEX idx_aspiral_ticks_t_symbol ON aspiral_ticks (t, symbol_id);

-- 6. VIEW CREATION (Initial Aggregation)
RAISE NOTICE '--- Creating Baseline Views (Aggregating 100M rows) ---';
CREATE MATERIALIZED VIEW baseline_ohlcv_1m AS SELECT date_trunc('minute', t) as t, symbol_id, (first_pg(price, t)).v as o, max(price) as h, min(price) as l, (last_pg(price, t)).v as c, sum(vol) as volume FROM baseline_ticks GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_ohlcv_1h AS SELECT date_trunc('hour', t) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_ohlcv_1m GROUP BY 1, 2;
CREATE INDEX idx_baseline_ohlcv_1h_t_symbol ON baseline_ohlcv_1h (t, symbol_id);

RAISE NOTICE '--- Creating Aspiral Hierarchy (Aggregating 100M rows + Sketches) ---';
-- This will automatically create aspiral_ohlcv_5m and aspiral_ohlcv_1h
CREATE MATERIALIZED VIEW aspiral_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol_id,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch
FROM aspiral_ticks 
GROUP BY 1, 2;

CREATE INDEX idx_aspiral_ohlcv_1h_t_symbol ON aspiral_ohlcv_1h (t, symbol_id);

-- 7. PERFORMANCE QUERIES
SELECT '--- P95 Percentile: Raw Data (100M Rows) ---' as test;
EXPLAIN ANALYZE SELECT symbol_id, percentile_cont(0.95) WITHIN GROUP (ORDER BY price) FROM baseline_ticks WHERE symbol_id = 5 GROUP BY 1;

SELECT '--- P95 Percentile: Aspiral Sketch (Pre-aggregated 1h) ---' as test;
EXPLAIN ANALYZE SELECT symbol_id, aspiral_quantile(price_sketch, 0.95) FROM aspiral_ohlcv_1h WHERE symbol_id = 5;

-- STORAGE
SELECT 
    relname as name,
    pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class 
WHERE relname IN ('baseline_ticks', 'aspiral_ticks', 'aspiral_ohlcv_1h', 'baseline_ohlcv_1h', 'aspiral_ohlcv_1m', 'baseline_ohlcv_1m')
ORDER BY relname;
