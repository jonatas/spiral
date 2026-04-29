-- Spiral: Master Walkthrough (Experimental Framework)
-- Run this script to see the masterpiece in action.
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- Stage 1: Zero-Config Ingestion
\echo '--- [STAGE 1] Zero-Config Ingestion ---'
DROP TABLE IF EXISTS sensor_data_1m CASCADE;
DROP TABLE IF EXISTS sensor_data_1h CASCADE;
DROP TABLE IF EXISTS sensor_data_1d CASCADE;
DROP TABLE IF EXISTS sensor_data CASCADE;

CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int,
    reading double precision -- Spiral: stats, ohlc
); 

-- Stage 2: Hierarchical Statistics
\echo ''
\echo '--- [STAGE 2] Hierarchical Statistics ---'
INSERT INTO sensor_data (t, reading, sensor_id)
SELECT '2025-01-01 00:00:00Z'::timestamptz + (n || ' seconds')::interval, random() * 100, (n % 10)
FROM generate_series(0, 10000) n;

\echo '-> Refreshing rollups...'
SELECT spiral_refresh('sensor_data_1m');

-- Stage 3: Transparent Query Acceleration
\echo ''
\echo '--- [STAGE 3] Transparent Query Acceleration ---'
-- Let's use the new spiral_explain to see the plan before executing
SELECT spiral_explain('SELECT sum(reading) FROM sensor_data WHERE t >= ''2025-01-01 00:00:00Z''::timestamptz AND t < ''2025-01-01 01:00:00Z''::timestamptz');

-- Stage 4: Multi-Hierarchy Join
\echo ''
\echo '--- [STAGE 4] Multi-Hierarchy Join ---'
DROP TABLE IF EXISTS weather_raw_1m CASCADE;
DROP TABLE IF EXISTS weather_raw_1h CASCADE;
DROP TABLE IF EXISTS weather_raw_1d CASCADE;
DROP TABLE IF EXISTS weather_raw CASCADE;

CREATE TABLE weather_raw (
    t timestamptz NOT NULL, 
    temp double precision -- Spiral: sum
); 

INSERT INTO weather_raw (t, temp)
SELECT '2025-01-01 00:00:00Z'::timestamptz + (n || ' seconds')::interval, random() * 30
FROM generate_series(0, 10000) n;

SELECT spiral_refresh('weather_raw_1m');

\echo '-> Joining two Spiral tables on time (Diagnostic Explain):'
SELECT spiral_explain('SELECT sum(s.reading + w.temp) FROM sensor_data s JOIN weather_raw w ON s.t = w.t WHERE s.t >= ''2025-01-01 00:00:00Z''::timestamptz AND s.t < ''2025-01-01 01:00:00Z''::timestamptz');

-- Stage 5: masterpiece API and Storage
\echo ''
\echo '--- [STAGE 5] Masterpiece API & Storage ---'
\echo '-> Spiral Status (sensor_data):'
SELECT spiral_status('sensor_data');

\echo '-> Epoch Conversions:'
SELECT '2025-01-01 12:00:00Z'::timestamptz as human, spiral_to_epoch('2025-01-01 12:00:00Z') as address;

\echo '-> Packing data into 30x smaller Block XOR format...'
SELECT spiral_pack_delta_blocks('sensor_data', 12345);

\echo '-> Storage footprint on disk:'
\! ls -lh /tmp/spiral_main/12345_blocks.bin

\echo ''
\echo '--- MASTER WALKTHROUGH COMPLETED ---'
