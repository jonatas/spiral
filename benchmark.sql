-- Aspiral vs. Plain PostgreSQL Benchmark
-- Comparing Storage, Index usage, and Performance for Time-Series Rollups.

DO $$
BEGIN
    RAISE NOTICE '--- Starting Aspiral Benchmark Setup ---';
END $$;

-- 1. CLEANUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS baseline_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;

-- 2. SETUP EXTENSION
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 3. BASELINE TABLES & VIEWS (PLAIN POSTGRESQL)
CREATE TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);
CREATE INDEX idx_baseline_ticks_t_symbol ON baseline_ticks (t, symbol_id);

-- Standard "First/Last" Aggregates for Baseline
-- Note: These are slower because they don't take time as an argument, 
-- but we will use the ORDER BY syntax which is the standard PG way.
-- However, to be fair and faster, we can define custom ones that take time.
CREATE TYPE time_value_pg AS (v double precision, t timestamptz);

CREATE OR REPLACE FUNCTION first_pg_sfunc(state time_value_pg, val double precision, ts timestamptz)
RETURNS time_value_pg AS $$
BEGIN
    IF state IS NULL OR ts < state.t THEN
        RETURN (val, ts);
    END IF;
    RETURN state;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

CREATE OR REPLACE FUNCTION last_pg_sfunc(state time_value_pg, val double precision, ts timestamptz)
RETURNS time_value_pg AS $$
BEGIN
    IF state IS NULL OR ts >= state.t THEN
        RETURN (val, ts);
    END IF;
    RETURN state;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

CREATE AGGREGATE first_pg(double precision, timestamptz) (
    sfunc = first_pg_sfunc,
    stype = time_value_pg
);

CREATE AGGREGATE last_pg(double precision, timestamptz) (
    sfunc = last_pg_sfunc,
    stype = time_value_pg
);

-- Baseline Materialized Views
CREATE MATERIALIZED VIEW baseline_ohlcv_1m AS
SELECT 
    date_trunc('minute', t) as t,
    symbol_id,
    (first_pg(price, t)).v as o,
    max(price) as h,
    min(price) as l,
    (last_pg(price, t)).v as c,
    sum(vol) as volume
FROM baseline_ticks
GROUP BY 1, 2;

CREATE MATERIALIZED VIEW baseline_ohlcv_5m AS
SELECT 
    date_trunc('minute', (t - ((extract(minute from t)::int % 5) || ' minutes')::interval)) as t,
    symbol_id,
    (first_pg(o, t)).v as o,
    max(h) as h,
    min(l) as l,
    (last_pg(c, t)).v as c,
    sum(volume) as volume
FROM baseline_ohlcv_1m
GROUP BY 1, 2;

CREATE MATERIALIZED VIEW baseline_ohlcv_1h AS
SELECT 
    date_trunc('hour', t) as t,
    symbol_id,
    (first_pg(o, t)).v as o,
    max(h) as h,
    min(l) as l,
    (last_pg(c, t)).v as c,
    sum(volume) as volume
FROM baseline_ohlcv_5m
GROUP BY 1, 2;

-- 4. ASPIRAL TABLES & VIEWS
CREATE TABLE aspiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);
CREATE INDEX idx_aspiral_ticks_t_symbol ON aspiral_ticks (t, symbol_id);

-- Create Aspiral Hierarchy
CREATE MATERIALIZED VIEW aspiral_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol_id,
    first(price, aspiral(t)) as o, 
    max(price) as h, 
    min(price) as l, 
    last(price, aspiral(t)) as c,
    sum(vol) as volume
FROM aspiral_ticks 
GROUP BY 1, 2;

-- 5. BENCHMARK EXECUTION
DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    ingest_duration_baseline interval;
    ingest_duration_aspiral interval;
    refresh_duration_baseline interval;
    refresh_duration_aspiral interval;
BEGIN
    -- A. Ingestion Baseline
    start_time := clock_timestamp();
    INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'),
        (i % 10) + 1,
        60000 + sin(i::float/100) * 1000 + random() * 100,
        (random() * 100)::int
    FROM generate_series(0, 999999) i;
    end_time := clock_timestamp();
    ingest_duration_baseline := end_time - start_time;
    
    -- B. Ingestion Aspiral
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'),
        (i % 10) + 1,
        60000 + sin(i::float/100) * 1000 + random() * 100,
        (random() * 100)::int
    FROM generate_series(0, 999999) i;
    end_time := clock_timestamp();
    ingest_duration_aspiral := end_time - start_time;

    -- C. Refresh Baseline
    start_time := clock_timestamp();
    REFRESH MATERIALIZED VIEW baseline_ohlcv_1m;
    REFRESH MATERIALIZED VIEW baseline_ohlcv_5m;
    REFRESH MATERIALIZED VIEW baseline_ohlcv_1h;
    end_time := clock_timestamp();
    refresh_duration_baseline := end_time - start_time;

    -- D. Refresh Aspiral
    start_time := clock_timestamp();
    REFRESH MATERIALIZED VIEW aspiral_ohlcv_1m; -- Automatically cascades
    end_time := clock_timestamp();
    refresh_duration_aspiral := end_time - start_time;

    -- E. Report Results
    RAISE NOTICE '--- INGESTION (1M Rows) ---';
    RAISE NOTICE 'Baseline: %', ingest_duration_baseline;
    RAISE NOTICE 'Aspiral:  %', ingest_duration_aspiral;
    RAISE NOTICE '';
    RAISE NOTICE '--- REFRESH (1m -> 5m -> 1h) ---';
    RAISE NOTICE 'Baseline: %', refresh_duration_baseline;
    RAISE NOTICE 'Aspiral:  %', refresh_duration_aspiral;
END $$;

-- 6. STORAGE & INDEX USAGE REPORT
SELECT 
    relname as name,
    pg_size_pretty(pg_table_size(oid)) as table_size,
    pg_size_pretty(pg_indexes_size(oid)) as index_size,
    pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class 
WHERE relname IN (
    'baseline_ticks', 'aspiral_ticks',
    'baseline_ohlcv_1m', 'baseline_ohlcv_5m', 'baseline_ohlcv_1h',
    'aspiral_ohlcv_1m', 'aspiral_ohlcv_1m_5m', 'aspiral_ohlcv_1m_1h'
)
ORDER BY relname;

-- 7. QUERY PERFORMANCE SAMPLES
SELECT '--- Query: Last 5 Hours of 1h Rollup (Baseline) ---' as test;
EXPLAIN ANALYZE SELECT * FROM baseline_ohlcv_1h WHERE symbol_id = 5 ORDER BY t DESC LIMIT 5;

SELECT '--- Query: Last 5 Hours of 1h Rollup (Aspiral) ---' as test;
EXPLAIN ANALYZE SELECT * FROM aspiral_ohlcv_1m_1h WHERE symbol_id = 5 ORDER BY t DESC LIMIT 5;
