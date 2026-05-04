LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE test_stats (
    t timestamptz NOT NULL,
    val double precision -- Spiral: stats
) WITH (spiral.frames = '1m');

INSERT INTO test_stats (t, val) VALUES
('2026-04-15 00:00:00Z', 10.0),
('2026-04-15 00:00:01Z', 20.0),
('2026-04-15 00:00:02Z', 30.0);

-- Trigger refresh to create 1m view
SELECT spiral_refresh('test_stats');

-- Check stats in 1m view
SELECT 
    round(spiral_stats_mean(val_stats)::numeric, 2) as mean,
    round(spiral_stats_variance(val_stats)::numeric, 2) as variance,
    round(spiral_stats_stddev(val_stats)::numeric, 2) as stddev
FROM test_stats_1m;

-- Add more data to verify incremental stats
INSERT INTO test_stats (t, val) VALUES
('2026-04-15 00:00:03Z', 40.0),
('2026-04-15 00:00:04Z', 50.0);

SELECT spiral_refresh('test_stats');

SELECT 
    round(spiral_stats_mean(val_stats)::numeric, 2) as mean,
    round(spiral_stats_variance(val_stats)::numeric, 2) as variance
FROM test_stats_1m;
