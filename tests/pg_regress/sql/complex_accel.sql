SET spiral.kickoff_date = '1970-01-01 00:00:00Z';
DROP TABLE IF EXISTS complex_accel_test CASCADE;
CREATE TABLE complex_accel_test (
    t timestamptz,
    tenant_id int,
    val_a double precision,
    val_b double precision,
    category text
);

-- Accelerate with multiple frames and columns
SELECT accelerate('complex_accel_test', '1m, 1h', ARRAY['tenant_id', 'category'], ARRAY['val_a stats', 'val_b stats']);

-- Insert data spanning multiple buckets
-- Bucket 1: 1970-01-01 00:00:00 to 00:01:00
INSERT INTO complex_accel_test VALUES 
('1970-01-01 00:00:10Z', 1, 10, 100, 'cpu'),
('1970-01-01 00:00:20Z', 1, 20, 200, 'cpu'),
('1970-01-01 00:00:30Z', 2, 30, 300, 'mem');

-- Bucket 2: 1970-01-01 00:01:00 to 00:02:00
INSERT INTO complex_accel_test VALUES 
('1970-01-01 00:01:10Z', 1, 40, 400, 'cpu'),
('1970-01-01 00:01:20Z', 2, 50, 500, 'mem');

-- Refresh first bucket to rollup
SELECT spiral_refresh('complex_accel_test_1m', 't < ''1970-01-01 00:01:00Z''');

-- Insert raw data for Bucket 3 (current)
INSERT INTO complex_accel_test VALUES 
('1970-01-01 00:02:10Z', 1, 60, 600, 'cpu');

-- MATRIX OF EXAMPLES

-- 1. Multiple aggregates, multiple columns, scoped filter
\echo '--- [MATRIX 1] Multi-agg, multi-col, scope filter ---'
EXPLAIN (VERBOSE, COSTS OFF)
SELECT category, COUNT(val_a), SUM(val_b), AVG(val_a + val_b) 
FROM complex_accel_test 
WHERE tenant_id = 1 AND t < '1970-01-01 00:03:00Z'
GROUP BY category;

SELECT category, COUNT(val_a), SUM(val_b), AVG(val_a + val_b) 
FROM complex_accel_test 
WHERE tenant_id = 1 AND t < '1970-01-01 00:03:00Z'
GROUP BY category;

-- 2. Expression in aggregate, mixed segments
-- NOTE: AVG(val_a * 2) should fall back if not explicitly mapped, or work if rewritten to spiral_avg(stats(val_a)) * 2?
-- Current implementation maps bare Var to stats. Complex expressions like val_a * 2 inside agg should fall back.
\echo '--- [MATRIX 2] Complex expression in aggregate (Fallback expected) ---'
EXPLAIN (VERBOSE, COSTS OFF)
SELECT SUM(val_a * 2), COUNT(*) 
FROM complex_accel_test;

SELECT SUM(val_a * 2), COUNT(*) 
FROM complex_accel_test;

-- 3. Complex AST: Aggregate in CTE, Join
\echo '--- [MATRIX 3] CTE + Join ---'
EXPLAIN (VERBOSE, COSTS OFF)
WITH agg_data AS (
    SELECT category, AVG(val_a) as avg_a
    FROM complex_accel_test
    WHERE t < '1970-01-01 00:03:00Z'
    GROUP BY category
)
SELECT a.*, b.val_b
FROM agg_data a
JOIN complex_accel_test b ON a.category = b.category
WHERE b.t >= '1970-01-01 00:02:00Z';

WITH agg_data AS (
    SELECT category, AVG(val_a) as avg_a
    FROM complex_accel_test
    WHERE t < '1970-01-01 00:03:00Z'
    GROUP BY category
)
SELECT a.*, b.val_b
FROM agg_data a
JOIN complex_accel_test b ON a.category = b.category
WHERE b.t >= '1970-01-01 00:02:00Z';

-- 4. Mathematical Integrity check: AVG over mixed segments
-- Rollup (B1, T1): [10, 20] -> count=2, sum=30, avg=15
-- Raw (B2, T1): [40] -> count=1, sum=40, avg=40
-- Raw (B3, T1): [60] -> count=1, sum=60, avg=60
-- Expected Total T1: count=4, sum=130, avg=32.5
\echo '--- [MATRIX 4] Mathematical Integrity (AVG mixed) ---'
SELECT COUNT(val_a), SUM(val_a), AVG(val_a)
FROM complex_accel_test
WHERE tenant_id = 1;

-- 5. MIN/MAX support
\echo '--- [MATRIX 5] MIN/MAX support ---'
EXPLAIN (VERBOSE, COSTS OFF)
SELECT MIN(val_a), MAX(val_b)
FROM complex_accel_test;

SELECT MIN(val_a), MAX(val_b)
FROM complex_accel_test;

DROP TABLE complex_accel_test CASCADE;
