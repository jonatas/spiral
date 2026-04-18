-- Aspiral vs. Plain PostgreSQL Comprehensive Benchmark
-- Comparing Storage, Index usage, and Performance for Time-Series Rollups and Binary Store.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;
DROP TYPE IF EXISTS time_value_pg CASCADE;

-- 2. SETUP EXTENSION
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 3. BASELINE SETUP
CREATE TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);
CREATE INDEX idx_baseline_ticks_t_symbol ON baseline_ticks (t, symbol_id);

CREATE TYPE time_value_pg AS (v double precision, t timestamptz);
CREATE OR REPLACE FUNCTION first_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts < state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE OR REPLACE FUNCTION last_pg_sfunc(state time_value_pg, val double precision, ts timestamptz) RETURNS time_value_pg AS $$
    BEGIN RETURN CASE WHEN state IS NULL OR ts >= state.t THEN (val, ts)::time_value_pg ELSE state END; END;
$$ LANGUAGE plpgsql IMMUTABLE;
CREATE AGGREGATE first_pg(double precision, timestamptz) (sfunc = first_pg_sfunc, stype = time_value_pg);
CREATE AGGREGATE last_pg(double precision, timestamptz) (sfunc = last_pg_sfunc, stype = time_value_pg);

CREATE MATERIALIZED VIEW baseline_ohlcv_1m AS SELECT date_trunc('minute', t) as t, symbol_id, (first_pg(price, t)).v as o, max(price) as h, min(price) as l, (last_pg(price, t)).v as c, sum(vol) as volume FROM baseline_ticks GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_ohlcv_5m AS SELECT date_trunc('minute', (t - ((extract(minute from t)::int % 5) || ' minutes')::interval)) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_ohlcv_1m GROUP BY 1, 2;
CREATE MATERIALIZED VIEW baseline_ohlcv_1h AS SELECT date_trunc('hour', t) as t, symbol_id, (first_pg(o, t)).v as o, max(h) as h, min(l) as l, (last_pg(c, t)).v as c, sum(volume) as volume FROM baseline_ohlcv_5m GROUP BY 1, 2;

CREATE INDEX idx_baseline_ohlcv_1h_t_symbol ON baseline_ohlcv_1h (t, symbol_id);

-- 4. ASPIRAL SETUP
CREATE TABLE aspiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);
CREATE INDEX idx_aspiral_ticks_t_symbol ON aspiral_ticks (t, symbol_id);

CREATE MATERIALIZED VIEW aspiral_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol_id,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch
FROM aspiral_ticks 
GROUP BY 1, 2;

-- Create indices on aspiral children manually (Aspiral doesn't do it yet)
CREATE INDEX idx_aspiral_ohlcv_1h_t_symbol ON aspiral_ohlcv_1h (t, symbol_id);

-- 5. BENCHMARK EXECUTION
DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    dur_ingest_base interval;
    dur_ingest_aspi interval;
    dur_refresh_base interval;
    dur_refresh_aspi interval;
    dur_pack_aspi interval;
    rows int := 200000;
BEGIN
    RAISE NOTICE '--- Starting Ingestion (200K Rows) ---';
    
    start_time := clock_timestamp();
    INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'), (i % 10), 60000 + sin(i::float/100)*1000 + random()*100, (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_ingest_base := clock_timestamp() - start_time;
    
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'), (i % 10), 60000 + sin(i::float/100)*1000 + random()*100, (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur_ingest_aspi := clock_timestamp() - start_time;

    RAISE NOTICE '--- Starting Refresh ---';
    
    start_time := clock_timestamp();
    REFRESH MATERIALIZED VIEW baseline_ohlcv_1m;
    REFRESH MATERIALIZED VIEW baseline_ohlcv_5m;
    REFRESH MATERIALIZED VIEW baseline_ohlcv_1h;
    dur_refresh_base := clock_timestamp() - start_time;

    start_time := clock_timestamp();
    REFRESH MATERIALIZED VIEW aspiral_ohlcv_1m;
    dur_refresh_aspi := clock_timestamp() - start_time;

    RAISE NOTICE '--- Testing Binary Pack (Main Store) ---';
    start_time := clock_timestamp();
    -- We use symbol_id as tenant_id and aspiral(t) as t (bigint)
    CREATE TEMP TABLE delta_tmp AS SELECT aspiral(t) as t, symbol_id as tenant_id, price FROM aspiral_ticks;
    PERFORM aspiral_pack_delta('delta_tmp', 999);
    dur_pack_aspi := clock_timestamp() - start_time;

    RAISE NOTICE 'RESULTS:';
    RAISE NOTICE 'Rows Processed:      %', rows;
    RAISE NOTICE 'Ingest Rate (Base):  % rows/s', round(rows / extract(epoch from dur_ingest_base));
    RAISE NOTICE 'Ingest Rate (Aspi):  % rows/s', round(rows / extract(epoch from dur_ingest_aspi));
    RAISE NOTICE 'Ingestion Baseline:  %', dur_ingest_base;
    RAISE NOTICE 'Ingestion Aspiral:   %', dur_ingest_aspi;
    RAISE NOTICE 'Refresh Baseline:    %', dur_refresh_base;
    RAISE NOTICE 'Refresh Aspiral:     %', dur_refresh_aspi;
    RAISE NOTICE 'Binary Pack (O(1)):  %', dur_pack_aspi;
END $$;

-- 6. STORAGE REPORT
SELECT 
    relname as name,
    pg_size_pretty(pg_table_size(oid)) as table_size,
    pg_size_pretty(pg_indexes_size(oid)) as index_size,
    pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class 
WHERE relname IN (
    'baseline_ticks', 'aspiral_ticks',
    'baseline_ohlcv_1m', 'baseline_ohlcv_5m', 'baseline_ohlcv_1h',
    'aspiral_ohlcv_1m', 'aspiral_ohlcv_5m', 'aspiral_ohlcv_1h'
)
ORDER BY relname;

-- 7. QUERY SAMPLES (Rollup vs. Raw)
SELECT '--- Query: 1h Rollup for Symbol 5 (Baseline View) ---' as test;
EXPLAIN ANALYZE SELECT * FROM baseline_ohlcv_1h WHERE symbol_id = 5 ORDER BY t DESC LIMIT 5;

SELECT '--- Query: 1h Rollup for Symbol 5 (Aspiral View) ---' as test;
EXPLAIN ANALYZE SELECT * FROM aspiral_ohlcv_1h WHERE symbol_id = 5 ORDER BY t DESC LIMIT 5;

SELECT '--- Query: 1h Rollup for Symbol 5 (RAW DATA - No View) ---' as test;
EXPLAIN ANALYZE 
SELECT 
    date_trunc('hour', t) as t,
    symbol_id,
    (first_pg(price, t)).v as o,
    max(price) as h,
    min(price) as l,
    (last_pg(price, t)).v as c,
    sum(vol) as volume
FROM baseline_ticks 
WHERE symbol_id = 5
GROUP BY 1, 2
ORDER BY t DESC LIMIT 5;

-- 8. PERCENTILE COMPARISON (Sketch vs. Raw)
SELECT '--- Query: P95 Price for Symbol 5 (Aspiral Sketch - 1h View) ---' as test;
EXPLAIN ANALYZE SELECT symbol_id, aspiral_quantile(price_sketch, 0.95) FROM aspiral_ohlcv_1h WHERE symbol_id = 5;

SELECT '--- Query: P95 Price for Symbol 5 (RAW DATA - percentile_cont) ---' as test;
EXPLAIN ANALYZE 
SELECT symbol_id, percentile_cont(0.95) WITHIN GROUP (ORDER BY price) 
FROM baseline_ticks 
WHERE symbol_id = 5
GROUP BY 1;

-- 9. AGGREGATION SCALING (Large Window)
SELECT '--- Query: Total Volume for all Symbols (Aspiral 1h View) ---' as test;
EXPLAIN ANALYZE SELECT symbol_id, sum(volume) FROM aspiral_ohlcv_1h GROUP BY 1;

SELECT '--- Query: Total Volume for all Symbols (RAW DATA) ---' as test;
EXPLAIN ANALYZE SELECT symbol_id, sum(vol) FROM baseline_ticks GROUP BY 1;

-- 10. O(1) READ ACCESS
SELECT '--- Query: O(1) Binary Read (Aspiral) ---' as test;
EXPLAIN ANALYZE SELECT aspiral_read_main(999, 100, 5);
