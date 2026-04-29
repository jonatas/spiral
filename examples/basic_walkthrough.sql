-- Spiral: The Complete Lifecycle Walkthrough
-- Sequential demonstration of storage, hierarchy, precision, and reactivity.

-- 1. Setup Kickoff
DROP EXTENSION IF EXISTS spiral CASCADE;
DROP SCHEMA IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral CASCADE;
SET spiral.kickoff_date = '2026-04-15';
DROP TABLE IF EXISTS asset_ticks CASCADE;
CREATE TABLE asset_ticks (t timestamptz NOT NULL, symbol text NOT NULL, price double precision, vol int);

-- 2. Create Intelligent Hierarchy
-- This spawns asset_ohlcv_5m and asset_ohlcv_1h automatically.
CREATE MATERIALIZED VIEW asset_ohlcv_1m WITH (spiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((spiral(t)/60)*60) as t, 
    symbol,
    first(price, spiral(t)) as o,
    max(price) as h,
    min(price) as l,
    last(price, spiral(t)) as c,
    sum(vol) as volume,
    spiral_sketch(price) as price_sketch 
FROM asset_ticks 
GROUP BY 1, 2;

-- 3. Ingest Realistic Data (2 hours of 10s ticks)
SELECT '--- Ingesting 2 hours of BTC price data ---' as msg;
INSERT INTO asset_ticks (t, symbol, price, vol) 
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i * interval '10 seconds'),
    'BTC',
    60000 + (sin(i::float/10) * 500) + (random() * 100), -- Oscillating price with noise
    (random() * 20)::int + 1
FROM generate_series(0, 719) s(i);

-- 4. Reactive Refresh (Cascading)
SELECT '--- Triggering Cascading Refresh ---' as msg;
REFRESH MATERIALIZED VIEW asset_ohlcv_1m;

-- 5. Verify OHLC Logic (5m frames)
SELECT '--- BTC OHLC (First 5 periods of 5m) ---' as msg;
SELECT t, symbol, round(o::numeric, 2) as o, round(h::numeric, 2) as h, round(l::numeric, 2) as l, round(c::numeric, 2) as c, volume 
FROM asset_ohlcv_5m 
ORDER BY t ASC LIMIT 5;

-- 6. Precise Hierarchical Percentiles (1h frames)
-- We calculate the p95 price for the entire hour from the T-Digest sketch.
SELECT '--- BTC 1h Statistics (from Sketches) ---' as msg;
SELECT 
    symbol, 
    round(spiral_quantile(price_sketch, 0.5)::numeric, 2) as median,
    round(spiral_quantile(price_sketch, 0.95)::numeric, 2) as p95,
    round(spiral_quantile(price_sketch, 0.99)::numeric, 2) as p99
FROM asset_ohlcv_1h;

-- 7. Surgical Backfill & Reactivity
SELECT '--- Simulating a data correction (backfill) ---' as msg;
-- We find a specific bucket and "break" the high price
UPDATE asset_ticks SET price = 85000.0 WHERE t = '2026-04-15 00:05:00Z' AND symbol = 'BTC';

-- Show that Spiral tracked exactly which bucket needs updating
SELECT '--- Spiral Changelog (Dirty Buckets) ---' as msg;
SELECT base_view, to_timestamptz(t_start) as bucket_t, scope_values FROM spiral.changelog;

-- Sync the backfill (Cascades automatically)
REFRESH MATERIALIZED VIEW asset_ohlcv_1m;

-- Verify the 5m and 1h views reflect the new high price
SELECT '--- Verification: 1h Max after backfill ---' as msg;
SELECT round(h::numeric, 2) as h_max_after_backfill FROM asset_ohlcv_1h;

-- 8. Planner Optimization Log
SELECT '--- Planner Optimization ---' as msg;
EXPLAIN (COSTS OFF) SELECT sum(h) FROM asset_ohlcv_1m WHERE t > '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:00:00Z';
