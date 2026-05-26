SET spiral.kickoff_date = '1970-01-01 00:00:00Z';
SET spiral.worker_enabled = off;
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS complex_accel_test CASCADE;
CREATE TABLE complex_accel_test (
    t timestamptz,
    tenant_id int,
    val_a double precision,
    val_b double precision,
    category text
);

-- Accelerate with multiple frames and columns
-- val_a -> stats formula
-- val_b -> ohlcv formula
SELECT accelerate('complex_accel_test', '1m, 1h', ARRAY['tenant_id', 'category'], ARRAY['val_a stats', 'val_b ohlcv']);

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

-- 1. Multiple aggregates, multiple columns, different formulas, scoped filter
\echo '--- [MATRIX 1] Multi-agg, multi-col, different formulas ---'
-- Should accelerate: val_a (stats), val_b (ohlcv: sum/max/min/first/last)
EXPLAIN (VERBOSE, COSTS OFF)
SELECT category, COUNT(val_a), SUM(val_a), AVG(val_a), MAX(val_b), first(val_b, t)
FROM complex_accel_test 
WHERE tenant_id = 1 AND t < '1970-01-01 00:03:00Z'
GROUP BY category;

SELECT category, COUNT(val_a), SUM(val_a), AVG(val_a), MAX(val_b), first(val_b, t)
FROM complex_accel_test 
WHERE tenant_id = 1 AND t < '1970-01-01 00:03:00Z'
GROUP BY category;

-- 2. Expression in aggregate (Fallback expected)
\echo '--- [MATRIX 2] Complex expression in aggregate (Fallback expected) ---'
EXPLAIN (VERBOSE, COSTS OFF)
SELECT SUM(val_a * 2), COUNT(*) 
FROM complex_accel_test;

SELECT SUM(val_a * 2), COUNT(*) 
FROM complex_accel_test;

-- 3. Complex AST: Aggregate in CTE
\echo '--- [MATRIX 3] CTE Acceleration ---'
EXPLAIN (VERBOSE, COSTS OFF)
WITH agg_data AS (
    SELECT category, AVG(val_a) as avg_a
    FROM complex_accel_test
    WHERE t < '1970-01-01 00:03:00Z'
    GROUP BY category
)
SELECT * FROM agg_data WHERE avg_a > 30;

WITH agg_data AS (
    SELECT category, AVG(val_a) as avg_a
    FROM complex_accel_test
    WHERE t < '1970-01-01 00:03:00Z'
    GROUP BY category
)
SELECT * FROM agg_data WHERE avg_a > 30;

-- 4. Mathematical Integrity check: Mixed segments (Rollup B1 + Raw B2 + Raw B3)
-- T1 data:
-- B1 (Rollup): [10, 20] -> count=2, sum=30
-- B2 (Raw): [40] -> count=1, sum=40
-- B3 (Raw): [60] -> count=1, sum=60
-- Expected Total T1: count=4, sum=130, avg=32.5
\echo '--- [MATRIX 4] Mathematical Integrity (AVG mixed) ---'
SELECT COUNT(val_a), SUM(val_a), AVG(val_a)
FROM complex_accel_test
WHERE tenant_id = 1;

-- 5. OHLCV verification
-- T1 val_b: [100, 200] in B1, [400] in B2, [600] in B3
-- Expected: open=100, high=600, low=100, close=600, volume=1300
\echo '--- [MATRIX 5] OHLCV verification ---'
SELECT first(val_b, t) as open, MAX(val_b) as high, MIN(val_b) as low, last(val_b, t) as close, SUM(val_b) as volume
FROM complex_accel_test
WHERE tenant_id = 1;

DROP TABLE complex_accel_test CASCADE;
