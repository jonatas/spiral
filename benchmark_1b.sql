-- Aspiral Scalability & Backfill Test (Final Verification)
-- Testing incremental refresh performance under heavy load and backfill.

-- 1. CLEANUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_1m CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_5m CASCADE;
DROP TABLE IF EXISTS aspiral_ohlcv_1h CASCADE;

-- 2. SETUP
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 3. UNLOGGED TABLES
CREATE UNLOGGED TABLE aspiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price double precision,
    vol int
);

-- 4. INITIAL BULK INGEST (1M rows)
DO $$
DECLARE
    rows int := 1000000; -- 1M
    start_time timestamptz;
    dur interval;
BEGIN
    RAISE NOTICE '--- Bulk Ingesting 1M Rows (Aspiral) ---';
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'),
        (i % 10),
        60000 + sin(i::float/1000)*1000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur := clock_timestamp() - start_time;
    RAISE NOTICE 'Bulk Ingest Done: % (% rows/s)', dur, round(rows / extract(epoch from dur));
END $$;

-- 5. INDEXING
\echo '--- Creating Indexes ---'
CREATE INDEX idx_aspiral_ticks_symbol_t ON aspiral_ticks (symbol_id, t);
CREATE INDEX idx_aspiral_ticks_aspiral_t ON aspiral_ticks (symbol_id, aspiral(t));

-- 6. HIERARCHY CREATION
\echo '--- Creating Aspiral Hierarchy ---'
-- Root as Table for visibility in the same transaction
CREATE UNLOGGED TABLE aspiral_ohlcv_1m AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol_id,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume
FROM aspiral_ticks 
GROUP BY 1, 2;

-- Register metadata manually since it's a table
SELECT aspiral_register_view('aspiral_ohlcv_1m', 'BASE', 0, 'aspiral_ticks', ARRAY['symbol_id']);
-- Create children
SELECT aspiral_create_hierarchy('aspiral_ohlcv_1m', '5m,1h', ARRAY['symbol_id']);

-- 7. BACKFILL TEST (1M rows in the past)
DO $$
DECLARE
    rows int := 1000000; -- 1M
    start_time timestamptz;
    dur interval;
BEGIN
    RAISE NOTICE '--- Backfilling 1M Rows (Past Data) ---';
    start_time := clock_timestamp();
    INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
    SELECT 
        '2026-04-14 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'),
        (i % 10),
        59000 + random()*100,
        (random()*100)::int
    FROM generate_series(0, rows-1) i;
    dur := clock_timestamp() - start_time;
    RAISE NOTICE 'Backfill Done: % (% rows/s)', dur, round(rows / extract(epoch from dur));
END $$;

-- 8. UPDATE TEST (100k rows modified)
\echo '--- Updating 100k Rows (Price Shock) ---'
\timing on
UPDATE aspiral_ticks SET price = price * 1.5 WHERE symbol_id = 1 AND t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 00:10:00Z';
\timing off

-- 9. INCREMENTAL REFRESH PERFORMANCE
\echo '--- Testing Incremental Refresh Performance ---'
-- Check dirty buckets count before refresh
SELECT count(*) as dirty_segments_before FROM aspiral.changelog;

\timing on
SELECT aspiral_refresh('aspiral_ohlcv_1m');
\timing off

-- 10. VERIFY RESULTS
\echo '--- Verification ---'
-- If incremental refresh worked, we should have data from both dates
SELECT symbol_id, min(t), max(t), count(*) FROM aspiral_ohlcv_1h GROUP BY 1 ORDER BY 1;

-- 11. CHANGELOG SIZE (Should be 0 after refresh)
\echo '--- Changelog Size (After Refresh) ---'
SELECT count(*) as dirty_segments_after FROM aspiral.changelog;
