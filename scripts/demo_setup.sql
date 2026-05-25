-- spiral_demo setup script
-- validates all SQL examples from the spiral-intro blog post

\echo '=== 1. Create sensor_data table ==='
CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision, -- Spiral: ohlcv
    humidity double precision,    -- Spiral: sum
    power_usage double precision  -- Spiral: stats
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'sensor_id'
);

\echo '=== 2. Check auto-created views ==='
SELECT view_name, parent_view, frame_seconds, scope_columns
FROM spiral.metadata ORDER BY frame_seconds;

\echo '=== 3. Insert raw data ==='
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage) VALUES
('2026-05-03 20:15:00'::timestamptz, 1, 22.5, 45.0, 100.5),
('2026-05-03 20:15:00'::timestamptz, 1, 22.7, 45.2, 101.0),
('2026-05-03 20:15:00'::timestamptz, 2, 19.5, 50.0, 80.0),
('2026-05-03 20:16:00'::timestamptz, 1, 23.0, 44.0, 105.0),
('2026-05-03 20:16:00'::timestamptz, 2, 19.8, 51.0, 82.0),
('2026-05-03 21:15:00'::timestamptz, 1, 25.0, 40.0, 110.0);

\echo '--- Raw data inserted:'
SELECT t, sensor_id, temperature, humidity, power_usage
FROM sensor_data ORDER BY t, sensor_id;

\echo '=== 4. Initial refresh ==='
SELECT spiral_refresh('sensor_data');

\echo '--- 1m rollup after refresh:'
SELECT t, sensor_id,
       temperature_ohlcv_o, temperature_ohlcv_h,
       temperature_ohlcv_l, temperature_ohlcv_c,
       humidity, power_usage_stats
FROM sensor_data_1m ORDER BY t, sensor_id;

\echo '--- 1h rollup after refresh:'
SELECT t, sensor_id,
       temperature_ohlcv_o, temperature_ohlcv_h,
       temperature_ohlcv_l, temperature_ohlcv_c,
       humidity, power_usage_stats
FROM sensor_data_1h ORDER BY t, sensor_id;

\echo '=== 5. Transparent query acceleration (max routes to _h sub-column) ==='
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz
  AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

\echo '=== 6. Insert late-arriving data ==='
INSERT INTO sensor_data (t, sensor_id, temperature, humidity, power_usage)
SELECT
    '2026-05-03 22:00:00'::timestamptz + (random() * 60 || ' minutes')::interval,
    id,
    20 + random() * 10,
    40 + random() * 20,
    90 + random() * 30
FROM generate_series(1, 2) AS id, generate_series(1, 60);

\echo '--- Changelog after late-arriving inserts:'
SELECT base_view, t_start, t_end, scope_values
FROM spiral.changelog ORDER BY t_start;

\echo '=== 7. Query with dirty data (3-tier slicing in action) ==='
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, sensor_id, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 19:00:00'::timestamptz
  AND t < '2026-05-03 23:00:00'::timestamptz
GROUP BY 1, 2;

\echo '=== 8. Heal orbits ==='
SELECT spiral_refresh('sensor_data');

\echo '--- Changelog after refresh (should be empty):'
SELECT base_view, t_start, t_end, scope_values
FROM spiral.changelog ORDER BY t_start;

\echo '=== 9. Delete and track dirty bucket ==='
DELETE FROM sensor_data
WHERE sensor_id = 1
  AND t >= '2026-05-03 20:15:00'::timestamptz
  AND t < '2026-05-03 20:25:00'::timestamptz;

\echo '--- Changelog after delete (only sensor_id=1 dirty):'
SELECT base_view, t_start, t_end, scope_values
FROM spiral.changelog ORDER BY t_start;

\echo '=== 10. Spiral status / lag ==='
SELECT * FROM spiral.status;

\echo '=== 11. Unbounded query acceleration (infers range from coarsest rollup) ==='
EXPLAIN (VERBOSE) SELECT sum(humidity) FROM sensor_data;
EXPLAIN (VERBOSE) SELECT max(temperature) FROM sensor_data;
EXPLAIN (VERBOSE) SELECT min(temperature) FROM sensor_data;

\echo '=== DONE: all examples validated ==='
