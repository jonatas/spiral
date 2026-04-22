-- Aspiral IoT Benchmark (TSBS-like Scenario)
-- Evaluating Aspiral for High-Frequency Telemetry with Hierarchy Statistics

-- 1. CLEANUP & SETUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS baseline_readings CASCADE;
DROP TABLE IF EXISTS baseline_devices CASCADE;
DROP TABLE IF EXISTS aspiral_readings CASCADE;
DROP TABLE IF EXISTS aspiral_devices CASCADE;

-- Also drop views created by magic comments (they are not owned by the extension)
DROP TABLE IF EXISTS aspiral_readings_ohlcv_1m CASCADE;
DROP TABLE IF EXISTS aspiral_readings_ohlcv_1h CASCADE;
DROP TABLE IF EXISTS aspiral_readings_ohlcv_1d CASCADE;
DROP TABLE IF EXISTS aspiral_readings_ohlcv_1mon CASCADE;

CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 2. SCHEMA DEFINITION
-- Baseline Tables (Standard PG)
CREATE TABLE baseline_devices (
    id int PRIMARY KEY,
    name text,
    fleet text,
    driver text,
    model text,
    device_version text,
    load_capacity double precision
);

CREATE TABLE baseline_readings (
    t timestamptz NOT NULL,
    device_id int REFERENCES baseline_devices(id),
    velocity double precision,
    fuel_consumption double precision,
    grade double precision,
    current_load double precision,
    status text
);

-- Aspiral Tables (Magic Comments)
CREATE TABLE aspiral_devices (
    id int PRIMARY KEY,
    name text,
    fleet text,
    driver text,
    model text,
    device_version text,
    load_capacity double precision
);

CREATE TABLE aspiral_readings (
    t timestamptz NOT NULL,
    device_id int REFERENCES aspiral_devices(id), -- Auto-detected as tenant!
    velocity double precision,       -- Aspiral: stats as velocity_stats
    fuel_consumption double precision, -- Aspiral: sum as fuel_consumption_sum, stats as fuel_consumption_stats
    grade double precision,          -- Aspiral: stats as grade_stats
    current_load double precision,   -- Aspiral: ohlc as current_load
    status text
) WITH (aspiral.frames='1m,1h,1d'); -- Specify frames explicitly

CREATE INDEX idx_aspiral_readings_t ON aspiral_readings (aspiral(t));

-- 3. DATA GENERATION
-- Seed 100 Devices
INSERT INTO baseline_devices (id, name, fleet, driver, model, device_version, load_capacity)
SELECT i, 'truck_' || i, 'fleet_' || (i % 10), 'driver_' || i, 'model_x', 'v1.2', 10000 + random() * 5000
FROM generate_series(1, 100) i;

INSERT INTO aspiral_devices SELECT * FROM baseline_devices;

-- Ingest 1M Rows (Telemetry)
DO $$
DECLARE
    rows int := 1000000;
    start_time timestamptz;
    dur_base interval;
    dur_aspi interval;
BEGIN
    RAISE NOTICE '--- Ingesting 1M IoT Readings (Baseline) ---';
    start_time := clock_timestamp();
    INSERT INTO baseline_readings (t, device_id, velocity, fuel_consumption, grade, current_load, status)
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'),
        (i % 100) + 1,
        60 + sin(i::float/100)*20 + random()*5,
        10 + random()*5,
        sin(i::float/500)*5,
        5000 + random()*1000,
        CASE WHEN (i % 1000) = 0 THEN 'warning' ELSE 'ok' END
    FROM generate_series(0, rows-1) i;
    dur_base := clock_timestamp() - start_time;
    RAISE NOTICE 'Baseline Ingest: % (% rows/s)', dur_base, round(rows / extract(epoch from dur_base));

    RAISE NOTICE '--- Ingesting 1M IoT Readings (Aspiral) ---';
    start_time := clock_timestamp();
    INSERT INTO aspiral_readings (t, device_id, velocity, fuel_consumption, grade, current_load, status)
    SELECT 
        '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.1 seconds'),
        (i % 100) + 1,
        60 + sin(i::float/100)*20 + random()*5,
        10 + random()*5,
        sin(i::float/500)*5,
        5000 + random()*1000,
        CASE WHEN (i % 1000) = 0 THEN 'warning' ELSE 'ok' END
    FROM generate_series(0, rows-1) i;
    dur_aspi := clock_timestamp() - start_time;
    RAISE NOTICE 'Aspiral Ingest:  % (% rows/s)', dur_aspi, round(rows / extract(epoch from dur_aspi));
END $$;

-- 4. REFRESH ASPIRAL HIERARCHY
\echo '--- Refreshing Aspiral Hierarchy ---'
\timing on
SELECT aspiral_refresh('aspiral_readings_ohlcv_1m');
\timing off

-- 5. BASELINE HIERARCHY (Manual)
\echo '--- Creating Baseline Views (1m, 1h) ---'
\timing on
CREATE MATERIALIZED VIEW baseline_readings_1m AS
SELECT 
    date_trunc('minute', t) as t,
    device_id,
    avg(velocity) as velocity_avg,
    sum(fuel_consumption) as fuel_sum,
    max(current_load) as load_max
FROM baseline_readings
GROUP BY 1, 2;

CREATE MATERIALIZED VIEW baseline_readings_1h AS
SELECT 
    date_trunc('hour', t) as t,
    device_id,
    avg(velocity_avg) as velocity_avg,
    sum(fuel_sum) as fuel_sum,
    max(load_max) as load_max
FROM baseline_readings_1m
GROUP BY 1, 2;
\timing off

-- 6. BENCHMARK QUERIES
\echo '--- Query 1: Fleet Average Velocity (Last 12 Hours) ---'
\timing on
-- Baseline
SELECT avg(velocity_avg) FROM baseline_readings_1h WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 12:00:00Z';
-- Aspiral
SELECT avg(aspiral_stats_mean(velocity_stats)) FROM aspiral_readings_ohlcv_1h WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 12:00:00Z';
\timing off

\echo '--- Query 2: Total Fuel Consumption per Fleet (Daily) ---'
\timing on
-- Baseline
SELECT d.fleet, sum(r.fuel_sum) 
FROM baseline_readings_1h r 
JOIN baseline_devices d ON r.device_id = d.id 
GROUP BY 1;
-- Aspiral
SELECT d.fleet, sum(r.fuel_consumption_sum) 
FROM aspiral_readings_ohlcv_1h r 
JOIN aspiral_devices d ON r.device_id = d.id 
GROUP BY 1;
\timing off

\echo '--- Query 3: Max Load Anomalies (All Time) ---'
\timing on
-- Baseline
SELECT device_id, max(load_max) FROM baseline_readings_1h GROUP BY 1 HAVING max(load_max) > 5900;
-- Aspiral
SELECT device_id, max(current_load_h) FROM aspiral_readings_ohlcv_1h GROUP BY 1 HAVING max(current_load_h) > 5900;
\timing off

-- 7. STORAGE COMPARISON
SELECT relname as name, pg_size_pretty(pg_total_relation_size(oid)) as total_size
FROM pg_class WHERE relname IN (
    'baseline_readings', 'aspiral_readings', 
    'baseline_readings_1h', 'aspiral_readings_ohlcv_1h',
    'aspiral_readings_ohlcv_1m', 'aspiral_readings_ohlcv_1d'
)
ORDER BY relname;
