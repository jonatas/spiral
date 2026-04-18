-- Aspiral: The Complete Lifecycle Walkthrough
-- Sequential demonstration of storage, hierarchy, precision, and reactivity.

-- 1. Setup Kickoff
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP SCHEMA IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral CASCADE;
SET aspiral.kickoff_date = '2026-04-15';
DROP TABLE IF EXISTS asset_ticks CASCADE;
CREATE TABLE asset_ticks (t timestamptz NOT NULL, symbol text NOT NULL, price double precision, vol int);

-- 2. Create Intelligent Hierarchy
-- This spawns asset_ohlcv_5m and asset_ohlcv_1h automatically.
CREATE MATERIALIZED VIEW asset_ohlcv_1m WITH (aspiral.frames='5m,1h') AS 
SELECT 
    to_timestamptz((aspiral(t)/60)*60) as t, 
    symbol,
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch 
FROM asset_ticks 
GROUP BY 1, 2;

-- 3. Ingest Data (3 historical rows, 1 "now" row)
INSERT INTO asset_ticks VALUES 
  ('2026-04-15 00:00:10Z', 'BTC', 60000.0, 1),
  ('2026-04-15 00:01:10Z', 'BTC', 61000.0, 2),
  ('2026-04-15 00:02:10Z', 'BTC', 62000.0, 1),
  (now(), 'BTC', 65000.0, 10);

-- 4. Reactive Refresh (Cascading)
SELECT '--- Triggering Cascading Refresh ---' as msg;
REFRESH MATERIALIZED VIEW asset_ohlcv_1m;

-- 5. Verify Closed-Frame Logic
-- h should be 62000 (The 65000 'now' bucket is still open and thus excluded)
SELECT '--- 5m Max (Excludes Open Buckets) ---' as msg;
SELECT t, symbol, h FROM asset_ohlcv_5m;

-- 6. Precise Hierarchical Percentiles
SELECT '--- 1h p95 (From Merged T-Digest) ---' as msg;
SELECT symbol, aspiral_quantile(price_sketch, 0.95) as p95 FROM asset_ohlcv_1h;

-- 7. Surgical Backfill
SELECT '--- Performing Backfill and Checking Changelog ---' as msg;
UPDATE asset_ticks SET price = 99999.9 WHERE t = '2026-04-15 00:00:10Z' AND symbol = 'BTC';

-- Show the specific scope-bucket flagged as dirty
SELECT * FROM aspiral.changelog;

-- Sync the backfill
REFRESH MATERIALIZED VIEW asset_ohlcv_1m;

-- Verify the 1h max reflects the backfill
SELECT h as max_after_backfill FROM asset_ohlcv_1h;

-- 8. Planner Optimization Log
SELECT '--- Triggering Planner Routing Hook ---' as msg;
SELECT sum(h) FROM asset_ohlcv_1m WHERE t > '2026-04-15 00:00:00Z' AND t < '2026-04-16 00:00:00Z';
