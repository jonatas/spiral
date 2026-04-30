-- ==============================================================================
-- Spiral Walkthrough: The WITH Syntax (Modern Setup)
-- ==============================================================================
-- This example demonstrates the easiest and most powerful way to set up Spiral
-- using PostgreSQL's native WITH (...) table options during CREATE TABLE.

-- Step 1: Clean up and initialize
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- Step 2: Create a table using the modern WITH syntax
--
-- The WITH clause tells Spiral to automatically track this table,
-- set up specific rollups ('1m' and '1h'), and isolate data by 'sensor_id'.
-- We use magic comments (-- Spiral: ...) to define how columns aggregate.
CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision, -- Spiral: ohlc as temp
    humidity double precision,    -- Spiral: sum, count
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
\d sensor_data_ohlcv_1m

-- Step 4: Insert some data spanning multiple timeframes
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage) VALUES
(now(), 1, 22.5, 45.0, 100.5),
(now(), 1, 22.7, 45.2, 101.0),
(now(), 2, 19.5, 50.0, 80.0),
(now() + interval '1 minute', 1, 23.0, 44.0, 105.0),
(now() + interval '1 minute', 2, 19.8, 51.0, 82.0),
(now() + interval '60 minutes', 1, 25.0, 40.0, 110.0);

-- Step 5: Incremental Refresh
-- When we refresh the 1-minute rollup, Spiral does it incrementally!
SELECT '--- Refreshing 1m View ---' AS step;
SELECT spiral_refresh('sensor_data_ohlcv_1m');

-- Refresh the 1-hour rollup. Spiral automatically cascades from the 1m rollup
-- because it knows about the hierarchy!
SELECT '--- Refreshing 1h View ---' AS step;
SELECT spiral_refresh('sensor_data_ohlcv_1h');

-- Step 6: Query the continuous rollups!
-- The column names are dynamically derived from the magic comments.
SELECT '--- 1 Minute Rollup Data ---' AS step;
SELECT 
    t, 
    sensor_id, 
    temp_o, temp_h, temp_l, temp_c,  -- From "ohlc as temp"
    humidity_sum, humidity_count,    -- From "sum, count"
    round(power_usage_mean::numeric, 2) AS avg_power -- From "stats"
FROM sensor_data_ohlcv_1m
ORDER BY t, sensor_id;

-- And the 1-hour rollup
SELECT '--- 1 Hour Rollup Data ---' AS step;
SELECT 
    t, 
    sensor_id, 
    temp_h AS hour_high, 
    temp_l AS hour_low,
    humidity_sum AS hour_humidity_total
FROM sensor_data_ohlcv_1h
ORDER BY t, sensor_id;

-- Step 7: Transparent Query Acceleration
-- Spiral intercepts queries to the base table and rewrites them
-- to use the pre-aggregated rollups seamlessly!
SELECT '--- Query Acceleration Plan ---' AS step;
EXPLAIN (COSTS OFF)
SELECT spiral(t, '1 hour') AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= now() - interval '1 hour' AND t < now() + interval '2 hours'
GROUP BY 1, 2;
