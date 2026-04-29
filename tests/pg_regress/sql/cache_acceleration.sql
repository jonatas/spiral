-- Testing the Ultimate Caching System (Transparent Query Acceleration)
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE cache_test (
    t timestamptz NOT NULL,
    tenant_id int,
    val double precision -- Spiral: sum, count, avg, min, max
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'tenant_id'
);

-- Ingest data for two hours
-- Hour 1: 10:00:00 to 11:00:00
-- Hour 2: 11:00:00 to 12:00:00
INSERT INTO cache_test (t, tenant_id, val) VALUES
('2026-04-15 10:00:05Z', 1, 10.0),
('2026-04-15 10:01:05Z', 1, 20.0),
('2026-04-15 10:59:05Z', 1, 30.0),
('2026-04-15 11:00:05Z', 1, 40.0),
('2026-04-15 11:05:05Z', 1, 50.0);

-- Materialize views
SELECT spiral_refresh('cache_test_ohlcv_1m');
SELECT spiral_refresh('cache_test_ohlcv_1h');

-- Verify they are clean (changelog should be empty for base_view)
SELECT count(*) FROM spiral.changelog WHERE base_view = 'cache_test';

-- 1. Test: Full Hour Query (Should use 1h rollup)
-- We expect the notice from the planner hook to show acceleration
SELECT sum(val), count(val), avg(val), min(val), max(val)
FROM cache_test
WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 11:00:00Z';

-- 2. Test: Range query covering partial buckets
-- Edge case: 10:00:30 to 11:05:00
-- Should use raw for [10:00:30, 10:01:00)
-- Should use 1m for [10:01:00, 11:00:00)
-- Should use 1h if possible, but here we have 1m buckets.
-- Actually, the algorithm prefers 1h if it fits.
SELECT sum(val), count(val)
FROM cache_test
WHERE t >= '2026-04-15 10:00:30Z' AND t < '2026-04-15 11:05:00Z';

-- 3. Test: Dirty Data Injection
-- Insert data into an existing bucket
INSERT INTO cache_test (t, tenant_id, val) VALUES
('2026-04-15 10:00:30Z', 1, 100.0);

-- Changelog should now have this bucket
SELECT count(*) FROM spiral.changelog WHERE base_view = 'cache_test';

-- Query again: The result should be correct (10+100+20+30) = 160
SELECT sum(val) FROM cache_test
WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 11:00:00Z';

-- 4. Test: CTE (Common Table Expression)
WITH hourly_stats AS (
    SELECT tenant_id, sum(val) as s
    FROM cache_test
    WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 12:00:00Z'
    GROUP BY tenant_id
)
SELECT * FROM hourly_stats;

-- 5. Test: Partial column selection
SELECT val FROM cache_test WHERE t = '2026-04-15 10:00:05Z';

-- 6. Test: Group By tenant
SELECT tenant_id, sum(val)
FROM cache_test
WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 12:00:00Z'
GROUP BY tenant_id;

-- 7. Test: Very large range covering multiple hours
-- Insert data for another hour
INSERT INTO cache_test (t, tenant_id, val) VALUES
('2026-04-15 12:00:05Z', 1, 60.0);
SELECT spiral_refresh('cache_test_ohlcv_1m');
SELECT spiral_refresh('cache_test_ohlcv_1h');

SELECT sum(val) FROM cache_test
WHERE t >= '2026-04-15 10:00:00Z' AND t < '2026-04-15 13:00:00Z';

-- Final verification against a standard table
CREATE TABLE baseline_test AS SELECT * FROM cache_test;
SELECT 
    (SELECT sum(val) FROM cache_test) = (SELECT sum(val) FROM baseline_test) as sum_matches,
    (SELECT count(val) FROM cache_test) = (SELECT count(val) FROM baseline_test) as count_matches;
