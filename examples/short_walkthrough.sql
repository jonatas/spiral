-- Spiral: Zero-Config Lifecycle Walkthrough
-- Demonstrating Magic Comments, Automated Hierarchy, and Backfill Reactivity.

-- 1. Setup
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2026-04-15';

-- 2. Create Table with Magic Comments
-- Spiral will automatically detect 't' as time and create the view hierarchy.
DROP TABLE IF EXISTS asset_ticks CASCADE;
CREATE TABLE asset_ticks (
    t timestamptz NOT NULL,
    symbol text NOT NULL,
    price double precision, -- Spiral: ohlc as p, sketch as p_sketch
    vol int                 -- Spiral: sum as volume
) WITH (
    spiral.frames = '1m,5m,1h', 
    spiral.tenant = 'symbol'
);

-- 3. Ingest Data (2 hours of BTC ticks)
INSERT INTO asset_ticks (t, symbol, price, vol) 
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i * interval '10 seconds'),
    'BTC',
    60000 + (sin(i::float/10) * 500) + (random() * 100),
    (random() * 20)::int + 1
FROM generate_series(0, 719) s(i);

-- Single Refresh triggers the entire pipeline
SELECT '--- Refreshing 1m view (Cascades to 5m, 1h) ---' as msg;
SELECT spiral_refresh('asset_ticks_ohlcv_1m');


-- 5. Verify Automated Hierarchy
-- View names are derived from the table name and the first frame.
SELECT '--- 5m BTC OHLC (Derived from Magic Comments) ---' as msg;
SELECT t, symbol, round(p_o::numeric, 2) as o, round(p_h::numeric, 2) as h, round(p_c::numeric, 2) as c, volume 
FROM asset_ticks_ohlcv_5m 
ORDER BY t ASC LIMIT 5;

-- 6. Precise Hierarchical Percentiles (1h)
SELECT '--- 1h BTC Statistics (from T-Digest Sketches) ---' as msg;
SELECT 
    symbol, 
    round(spiral_quantile(p_sketch, 0.95)::numeric, 2) as p95,
    round(spiral_quantile(p_sketch, 0.99)::numeric, 2) as p99
FROM asset_ticks_ohlcv_1h;

-- 7. Surgical Backfill
SELECT '--- Backfill: Correcting an anomaly ---' as msg;
UPDATE asset_ticks SET price = 85000.0 WHERE t = '2026-04-15 00:05:00Z' AND symbol = 'BTC';

-- Verify Spiral tracked the dirty bucket
SELECT '--- Changelog Check ---' as msg;
SELECT base_view, to_timestamptz(t_start) as bucket_t FROM spiral.changelog;

-- Sync the backfill
SELECT spiral_refresh('asset_ticks_ohlcv_1m');

-- Verify 1h view reflects the fix
SELECT '--- Post-Backfill Check (1h Max) ---' as msg;
SELECT round(p_h::numeric, 2) as p_h_after_backfill FROM asset_ticks_ohlcv_1h;
