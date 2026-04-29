-- Spiral: The PostgreSQL Hacker's Tour
-- This script demonstrates the internals of the Spiral framework.

SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- Step 1: Automated Hierarchy & Tenant Detection
-- Spiral tracks time and tenant dimensions to build high-performance rollups.
\echo '--- [1] Automated Hierarchy & Tenant Detection ---'
SET spiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS sensors CASCADE;
CREATE TABLE sensors (id int PRIMARY KEY);
INSERT INTO sensors VALUES (1),(2),(3);

DROP TABLE IF EXISTS sensor_readings CASCADE;
CREATE TABLE sensor_readings (
    t timestamptz NOT NULL,
    sensor_id int REFERENCES sensors(id),
    voltage double precision,
    current double precision
);

-- Manual Registration (demonstrating the API)
SELECT spiral_register_view('sensor_readings_1m', 'BASE', 60, 'sensor_readings', ARRAY['sensor_id']);
SELECT spiral_register_view('sensor_readings_1h', 'sensor_readings_1m', 3600, 'sensor_readings', ARRAY['sensor_id']);

-- Inspect the metadata catalog
\echo '-> Metadata registered in Spiral Catalog:'
SELECT view_name, parent_view, frame_seconds, scope_columns 
FROM spiral.metadata 
WHERE base_view = 'sensor_readings' 
ORDER BY frame_seconds;

-- Step 2: Statement-Level Change Tracking
-- Spiral uses Transition Tables (REFERENCING NEW TABLE) to capture batch changes.
\echo ''
\echo '--- [2] Statement-Level Change Tracking ---'
INSERT INTO sensor_readings (t, sensor_id, voltage, current)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (n || ' seconds')::interval,
    (n % 3) + 1, 
    50 + sin(n::float/100)*10,
    10 + cos(n::float/100)*2
FROM generate_series(0, 500) n;

\echo '-> Spiral Changelog (dirty buckets tracked):'
SELECT base_view, t_start, t_end FROM spiral.changelog;

-- Step 3: Incremental View Maintenance (IVM)
-- Cascading refresh updates the entire hierarchy using MERGE logic.
\echo '-> Performing Incremental Refresh...'
SELECT spiral_refresh('sensor_readings_1m');

-- Step 4: Hierarchical Rollups
\echo ''
\echo '--- [4] Hierarchical Rollups: Heuristic Column Mapping ---'
-- Spiral maps columns using heuristics or manual metadata.
SELECT 
    t, sensor_id,
    round(voltage::numeric, 2) as voltage_sum,
    round(current::numeric, 2) as current_sum
FROM sensor_readings_1h 
ORDER BY t, sensor_id;

\echo '-> Internal Source Mapping (how Spiral knows what is what):'
SELECT base_column, formula, mat_column 
FROM spiral.sources 
WHERE view_name = 'sensor_readings_1h';

-- Step 5: Transparent Query Acceleration
-- The Planner Hook intercepts raw table queries and grafts rollup subqueries.
\echo ''
\echo '--- [5] Transparent Query Acceleration ---'
\echo '-> Diagnostic Slicing Plan:'
SELECT spiral_explain('SELECT sum(voltage) FROM sensor_readings WHERE t >= ''2026-04-15 00:00:00Z'' AND t < ''2026-04-15 01:00:00Z''');

\echo '-> Subquery Grafting in the Plan:'
EXPLAIN (COSTS OFF) 
SELECT sum(voltage) FROM sensor_readings 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:00:00Z';

-- Step 6: Z-Order Multi-Dimensional Clustering
\echo ''
\echo '--- [6] Z-Order Clustering ---'
SELECT cluster_table('sensor_readings_1m', 't', ARRAY['sensor_id']);

\echo '-> Bit-interleaved Index Value (on rollup):'
SELECT t, sensor_id, spiral_zorder(spiral(t), ARRAY[sensor_id::text])::bit(64) as z_bits
FROM sensor_readings_1m LIMIT 3;

\echo ''
\echo '--- TOUR COMPLETED ---'
