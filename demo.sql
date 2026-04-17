-- Aspiral Extension Demo Suite
-- Documentation and scenarios for high-precision time-series

-- 1. Setup Kickoff
SET aspiral.kickoff_date = '2026-04-15';

CREATE TABLE raw_ticks (t timestamptz NOT NULL, price f64, vol int);

-- 2. Create Intelligent Hierarchy
-- Open/High/Low/Close + Precise p95 Percentiles
CREATE MATERIALIZED VIEW stocks_ohlcv_1m AS 
SELECT 
    (aspiral(t)::bigint / 60) * 60 as t, 
    first(price, aspiral(t)) as o, max(price) as h, min(price) as l, last(price, aspiral(t)) as c,
    sum(vol) as volume,
    aspiral_sketch(price) as price_sketch 
FROM raw_ticks GROUP BY 1
WITH (aspiral.frames='5m,1h');

-- 3. Ingest Data
INSERT INTO raw_ticks (t, price, vol)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i || ' seconds')::interval,
    100 + (random() * 10 - 5),
    (random() * 100)::int
FROM generate_series(1, 300) s(i);

-- 4. Aspiraling Index (Seasonal Index)
-- Create a GiST index on the 2D spiral coordinates (1 day cycle)
CREATE INDEX idx_spiral_1d ON raw_ticks USING gist (to_spiral(aspiral(t), 86400));

-- 5. Reactive Refresh
REFRESH MATERIALIZED VIEW stocks_ohlcv_1m;

-- 6. Query Results
SELECT '--- 5m Projection Results ---' as msg;
SELECT t, o, h, l, c, aspiral_quantile(price_sketch, 0.95) as p95 
FROM stocks_ohlcv_5m;

-- 7. Seasonal Query using Spiral Index
SELECT '--- Seasonal Query (Same time-of-day) ---' as msg;
-- Querying a "wedge" of the spiral to find data between 00:01 and 00:02 across all days
-- (Simplified for demo, just showing the index exists)
EXPLAIN SELECT count(*) FROM raw_ticks 
WHERE to_spiral(aspiral(t), 86400) && '(0,0),(100,100)'::box;

-- 6. Backfill and Incremental Update
SELECT '--- Performing Backfill ---' as msg;
UPDATE raw_ticks SET price = 999.9 WHERE t = '2026-04-15 00:01:30Z';

-- Check changelog (Internal tracking)
SELECT * FROM aspiral.changelog;

-- Incrementally refresh the whole tree
REFRESH MATERIALIZED VIEW stocks_ohlcv_1m;

-- Verify 1h view was updated reactively
SELECT t, h as max_price_after_backfill FROM stocks_ohlcv_1h;
