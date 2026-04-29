-- Spiral Walkthrough: Multi-Level Hierarchical Query Acceleration
-- Use this script in psql to see the "Ultimate Caching System" in action.

SET client_min_messages TO NOTICE;
LOAD 'spiral';

-- 1. Look at the Lineage Registry
-- This registry maps base table columns to their materialized counterparts across ALL tiers.
\echo '--- SPIRAL LINEAGE REGISTRY ---'
SELECT view_name, base_column, formula, mat_column FROM spiral.sources ORDER BY view_name, mat_column;

-- 2. Look at the metadata
-- This tracks the hierarchy and parent-child relationships.
\echo '--- SPIRAL HIERARCHY ---'
SELECT view_name, parent_view, frame_seconds FROM spiral.metadata;

-- 3. Transparent Query Acceleration (The Magic)
-- We will query the RAW table, but watch how Spiral rewrites it.
\echo '--- EXPLAIN: Aggregating 1.5 Hours of data ---'
\echo 'Range: 2026-04-15 00:00:00 to 01:30:05'
EXPLAIN (VERBOSE, COSTS OFF) 
SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:30:05Z';

-- 4. Correctness Guarantee: Dirty Regions
-- Let's manually "dirty" a region by inserting new data into a previously materialized hour.
\echo '--- DIRTYING DATA (Inserting into materialized range) ---'
INSERT INTO stress_raw (t, val) VALUES ('2026-04-15 00:30:00Z', 1000);

-- Now, watch the EXPLAIN again. 
-- Spiral will detect the "dirty" minute and fall back to the raw table ONLY for that minute,
-- while still using Hourly/Minutely rollups for the clean regions!
\echo '--- EXPLAIN: Slicing around Dirty Data ---'
EXPLAIN (VERBOSE, COSTS OFF) 
SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:30:05Z';

-- 5. Final Result Check
-- Verification that the sum is correct including the dirty data.
\echo '--- FINAL ACCELERATED RESULT ---'
SELECT sum(val) FROM stress_raw 
WHERE t >= '2026-04-15 00:00:00Z' AND t < '2026-04-15 01:30:05Z';
