LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

-- Test table with various magic comments
CREATE TABLE sensors (
    t timestamptz NOT NULL,
    sensor_id int, -- Spiral: count as total_readings
    voltage double precision, -- Spiral: ohlcv as v, stats as v_stats
    current double precision, -- Spiral: stats
    status_code int           -- Spiral: count
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'sensor_id'
);

-- Check if views were created
SELECT view_name, parent_view, frame_seconds, base_view, scope_columns 
FROM spiral.metadata 
ORDER BY view_name;

-- Ingest some data
INSERT INTO sensors (t, sensor_id, voltage, current, status_code) VALUES
('2026-04-15 00:00:10Z', 1, 120.5, 1.2, 200),
('2026-04-15 00:00:20Z', 1, 120.6, 1.3, 200),
('2026-04-15 00:01:10Z', 1, 120.7, 1.4, 200),
('2026-04-15 00:00:15Z', 2, 220.1, 0.5, 200);

-- Refresh the views
SELECT spiral_refresh('sensors');

-- Check 1m view
SELECT t, sensor_id, v, current, status_code
FROM sensors_1m
ORDER BY t, sensor_id;

-- Check 1h view (should be automatically updated by cascading refresh)
SELECT t, sensor_id, v, current, status_code
FROM sensors_1h
ORDER BY t, sensor_id;
