
-- Reproduction for multi-segment filters
DROP EXTENSION IF EXISTS spiral CASCADE;
DROP TABLE IF EXISTS sensor_data CASCADE;
CREATE EXTENSION spiral;

CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision -- Spiral: ohlcv
) WITH (
    spiral.frames = '1h'
);

-- Insert data for 2 non-contiguous hours
INSERT INTO sensor_data (t, sensor_id, temperature) VALUES
('2026-05-03 10:30:00'::timestamptz, 1, 20.0),
('2026-05-03 12:30:00'::timestamptz, 1, 22.0);

-- Refresh
SELECT spiral_refresh('sensor_data');

-- Now "dirty" one middle hour and one later hour
INSERT INTO sensor_data (t, sensor_id, temperature) VALUES
('2026-05-03 11:30:00'::timestamptz, 1, 21.0),
('2026-05-03 13:30:00'::timestamptz, 1, 23.0);

-- Query across all 4 hours.
-- 10:00 and 12:00 are clean (sensor_data_1h)
-- 11:00 and 13:00 are dirty (sensor_data)
SET spiral.enable_planner_hook = on;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t) AS hour, max(temperature)
FROM sensor_data
WHERE t >= '2026-05-03 10:00:00'::timestamptz AND t < '2026-05-03 14:00:00'::timestamptz
GROUP BY 1 ORDER BY 1;
