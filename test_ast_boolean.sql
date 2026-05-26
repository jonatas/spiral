
-- Test for boolean logic correctness in AST Catalog
-- Ensure clean state
DROP EXTENSION IF EXISTS spiral CASCADE;
-- Re-create
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS sensor_data CASCADE;

CREATE TABLE sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    val double precision -- Spiral: sum
) WITH (
    spiral.frames = '1h'
);

INSERT INTO sensor_data (t, sensor_id, val) VALUES
('2026-05-03 10:30:00'::timestamptz, 1, 10.0),
('2026-05-03 11:30:00'::timestamptz, 1, 20.0);

SELECT spiral_refresh('sensor_data');

SET spiral.enable_planner_hook = on;

-- 1. Standard AND (Should accelerate)
SELECT '--- Test 1: AND (Should Accelerate) ---' AS test;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t), sum(val)
FROM sensor_data
WHERE t >= '2026-05-03 10:00:00'::timestamptz AND t < '2026-05-03 12:00:00'::timestamptz
GROUP BY 1;

-- 2. OR clause (Should NOT accelerate the OR branch improperly)
-- Previous implementation would have incorrectly treated this as AND
SELECT '--- Test 2: OR (Should NOT improperly accelerate) ---' AS test;
EXPLAIN (VERBOSE)
SELECT date_trunc('hour', t), sum(val)
FROM sensor_data
WHERE t >= '2026-05-03 10:00:00'::timestamptz OR sensor_id = 999
GROUP BY 1;
