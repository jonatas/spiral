-- ==============================================================================
-- Spiral Walkthrough: The WITH Syntax (Modern Setup)
-- ==============================================================================
-- This example demonstrates the easiest and most powerful way to set up Spiral
-- using PostgreSQL's native WITH (...) table options during CREATE TABLE.

-- Step 1: Clean up and initialize
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;
DROP TABLE IF EXISTS sensor_data CASCADE;
DROP TABLE IF EXISTS sensor_data_1m CASCADE;
DROP TABLE IF EXISTS sensor_data_1h CASCADE;

-- Step 2: Create a table using the modern WITH syntax
--
-- The WITH clause tells Spiral to automatically track this table,
-- set up specific rollups ('1m' and '1h'), and isolate data by 'sensor_id'.
-- We use magic comments (-- Spiral: ...) to define how columns aggregate.
CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision, -- Spiral: ohlcv
    humidity double precision,    -- Spiral: sum
    power_usage double precision  -- Spiral: stats
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'sensor_id'
);

-- Step 3: Behind the Scenes
-- Because of the WITH clause, Spiral immediately registered the table 
-- and created the hierarchical views upon table creation!
SELECT '--- Auto-created Rollup Views ---' AS step;
SELECT view_name, parent_view, frame_seconds, scope_columns 
FROM spiral.metadata 
ORDER BY frame_seconds;

-- Notice how it used the magic comments to build the view structure
SELECT '--- View Schema ---' AS step;
\d sensor_data_1m

-- Step 4: Insert some data spanning multiple timeframes
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage) VALUES
('2026-05-03 20:15:00'::timestamptz, 1, 22.5, 45.0, 100.5),
('2026-05-03 20:15:00'::timestamptz, 1, 22.7, 45.2, 101.0),
('2026-05-03 20:15:00'::timestamptz, 2, 19.5, 50.0, 80.0),
('2026-05-03 20:16:00'::timestamptz, 1, 23.0, 44.0, 105.0),
('2026-05-03 20:16:00'::timestamptz, 2, 19.8, 51.0, 82.0),
('2026-05-03 21:15:00'::timestamptz, 1, 25.0, 40.0, 110.0);

-- Step 5: Incremental Refresh
-- When we refresh the 1-minute rollup, Spiral does it incrementally!
SELECT '--- Refreshing 1m View ---' AS step;
SELECT spiral_refresh('sensor_data');

-- Refresh the 1-hour rollup. Spiral automatically cascades from the 1m rollup
-- because it knows about the hierarchy!
SELECT '--- Refreshing 1h View ---' AS step;
-- SELECT spiral_refresh('sensor_data_1h');

-- Step 6: Query the continuous rollups!
-- The column names are dynamically derived from the magic comments.
SELECT '--- 1 Minute Rollup Data ---' AS step;
SELECT 
    t, 
    sensor_id, 
    spiral_ohlcv_open(temperature) as temperature_ohlcv_o,
    spiral_ohlcv_high(temperature) as temperature_ohlcv_h,
    spiral_ohlcv_low(temperature) as temperature_ohlcv_l,
    spiral_ohlcv_close(temperature) as temperature_ohlcv_c,
    humidity,
    power_usage as power_usage_stats
FROM sensor_data_1m
ORDER BY t, sensor_id;

SELECT '--- 1 Hour Rollup Schema ---' AS step;
\d sensor_data_1h

-- And the 1-hour rollup
SELECT '--- 1 Hour Rollup Data ---' AS step;
SELECT * FROM sensor_data_1h ORDER BY t, sensor_id;

-- Step 7: Transparent Query Acceleration
-- Spiral intercepts queries to the base table and rewrites them
-- to use the pre-aggregated rollups seamlessly!
SELECT '--- Query Acceleration Plan ---' AS step;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

-- Step 8: Simulating New Data Arrival
-- Let's insert another hour of data for all tenants
SELECT '--- Inserting Late Data ---' AS step;
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage)
SELECT 
    '2026-05-03 22:00:00'::timestamptz + (random() * 60 || ' minutes')::interval,
    id,
    20 + random() * 10,
    40 + random() * 20,
    90 + random() * 30
FROM generate_series(1, 2) AS id, generate_series(1, 60);

-- Step 9: Inspect Changelog
-- Spiral tracks exactly which time buckets and tenants are "dirty"
SELECT '--- Inspecting Changelog ---' AS step;
SELECT base_view, t_start, t_end, scope_values FROM spiral.changelog ORDER BY t_start;

-- Step 10: Query Acceleration with Dirty Data
-- Spiral's planner is smart enough to know about the dirty data.
-- It will slice the query: clean segments go to the rollup, dirty segments go to the raw table!
SELECT '--- Acceleration Plan (Dirty Data) ---' AS step;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

-- Step 11: Refresh and Re-plan
-- Now we heal the dirty buckets. Notice how it only processes the changes!
SELECT '--- Refreshing (Healing) ---' AS step;
SELECT spiral_refresh('sensor_data');

SELECT '--- Acceleration Plan (Clean Data) ---' AS step;
-- The entire time range is now clean, so it uses 100% rollups!
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

-- Step 12: Simulating Deletions
SELECT '--- Deleting Data ---' AS step;
DELETE FROM sensor_data WHERE sensor_id = 1 AND t >= '2026-05-03 20:15:00'::timestamptz AND t < '2026-05-03 20:25:00'::timestamptz;

-- Check changelog again
SELECT '--- Inspecting Changelog After Delete ---' AS step;
SELECT base_view, t_start, t_end, scope_values FROM spiral.changelog ORDER BY t_start;

SELECT '--- Acceleration Plan (After Delete) ---' AS step;
-- The planner isolates the dirty fallback to ONLY sensor_id = 1 for that specific time bucket!
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

SELECT '--- Final Refresh ---' AS step;
SELECT spiral_refresh('sensor_data');

SELECT '--- Final Acceleration Plan ---' AS step;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;
