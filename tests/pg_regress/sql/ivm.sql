LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE metrics (
    t timestamptz NOT NULL,
    device_id text NOT NULL,
    val double precision -- Spiral: ohlcv, sum
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'device_id'
);

-- Ingest initial data
INSERT INTO metrics (t, device_id, val) VALUES
('2026-04-15 10:00:05Z', 'A', 10.0),
('2026-04-15 10:00:55Z', 'A', 20.0);

-- Refresh
SELECT spiral_refresh('metrics');

-- Check initial state
SELECT t, device_id, val 
FROM metrics_1m;

-- Update data (Backfill)
UPDATE metrics SET val = 15.0 WHERE t = '2026-04-15 10:00:05Z' AND device_id = 'A';

-- Check changelog
SELECT base_view, to_timestamptz(t_start), to_timestamptz(t_end), scope_values 
FROM spiral.changelog;

-- Refresh again
SELECT spiral_refresh('metrics');

-- Check updated state
SELECT t, device_id, val 
FROM metrics_1m;

-- Delete data
DELETE FROM metrics WHERE t = '2026-04-15 10:00:55Z' AND device_id = 'A';

-- Refresh again (incremental should handle deletion of one row)
SELECT spiral_refresh('metrics');

-- Check state (should have 1 row left)
SELECT t, device_id, val FROM metrics_1m;

-- Delete remaining data for that bucket
DELETE FROM metrics WHERE device_id = 'A';

-- Check changelog
SELECT base_view, to_timestamptz(t_start), to_timestamptz(t_end), scope_values 
FROM spiral.changelog;

-- Refresh again (incremental should handle deletion of ALL rows in bucket)
SELECT spiral_refresh('metrics');

-- Check final state (should be empty)
SELECT t, device_id, val 
FROM metrics_1m;
