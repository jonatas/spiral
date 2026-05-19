LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE test_tdigest (
    t timestamptz NOT NULL,
    val double precision -- Spiral: tdigest as td, stats
) WITH (spiral.frames = '1m');

INSERT INTO test_tdigest (t, val) SELECT
    '2026-04-15 00:00:00Z'::timestamptz + (i || ' seconds')::interval,
    i::double precision
FROM generate_series(0, 100) s(i);

SELECT spiral_refresh('test_tdigest');

-- Check p50 (should be around 50)
SELECT
    spiral_quantile(td, 0.5) as p50,
    round(spiral_stats_mean(val_stats)::numeric, 2) as mean
FROM test_tdigest_1m;

-- Verify aliasing and multiple formulas
SELECT column_name
FROM information_schema.columns
WHERE table_name = 'test_tdigest_1m'
ORDER BY column_name;
