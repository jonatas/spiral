-- Aspiral Edge Case & Inconsistency Probe
SET client_min_messages TO NOTICE;
LOAD 'aspiral';

-- ============================================================================
-- SCENARIO 1: Unmapped Aggregates
-- ============================================================================
\echo '--- [PROBE 1] Unmapped Aggregates (STDDEV) ---'
-- Standard deviation is not in our map_agg_inner. 
-- Expectation: It should either fallback to raw or possibly fail if it tries to 
-- apply stddev to a sum column.
EXPLAIN (COSTS OFF) SELECT stddev(val) FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:00:00Z';

-- ============================================================================
-- SCENARIO 2: Filter Push-down on Non-Scope Columns
-- ============================================================================
\echo '--- [PROBE 2] Filter Push-down on Non-Scope Columns ---'
-- If we filter by 'val > 50', this filter needs to be inside the UNION ALL subquery
-- to be efficient, but currently we only project columns, we don't necessarily 
-- push non-time filters into the sub-leafs.
EXPLAIN (COSTS OFF) SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:00:00Z'
AND val > 50;

-- ============================================================================
-- SCENARIO 3: Non-Time Join
-- ============================================================================
\echo '--- [PROBE 3] Joining on Value instead of Time ---'
-- Joining on a non-time column. Constraint propagation should NOT happen here.
EXPLAIN (COSTS OFF) 
SELECT sum(s.val) 
FROM stress_raw s
JOIN regular_table r ON s.val::int = r.val
WHERE s.t >= '2026-04-15 00:00:00Z' AND s.t < '2026-04-15 01:00:00Z';

-- ============================================================================
-- SCENARIO 4: CTE Wrapping
-- ============================================================================
\echo '--- [PROBE 4] CTE Wrapping ---'
-- Querying through a CTE. Our planner hook iterates through rtable, 
-- so it SHOULD find and accelerate the table inside the CTE.
EXPLAIN (COSTS OFF)
WITH data AS (
    SELECT val, t FROM stress_raw WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:00:00Z'
)
SELECT sum(val) FROM data;
