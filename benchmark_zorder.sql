-- Z-Order vs Composite Index Benchmark
CREATE EXTENSION IF NOT EXISTS aspiral;

DROP TABLE IF EXISTS zorder_test;
CREATE TABLE zorder_test (
    t timestamptz NOT NULL,
    org_id int NOT NULL,
    user_id int NOT NULL,
    val double precision
);

-- Generate data: 100 Orgs, 100 Users per Org, over 10 days
INSERT INTO zorder_test (t, org_id, user_id, val)
SELECT 
    '2026-04-15'::timestamptz + (i * interval '1 minute'),
    (i % 100), -- org_id
    ((i / 100) % 100), -- user_id
    random()
FROM generate_series(0, 99999) i;

-- 1. Standard Composite Index (Good for Org, bad for Time-only)
CREATE INDEX idx_composite ON zorder_test (org_id, t);

-- 2. Z-Order Index (Fair for both)
-- We cast to timestamp without time zone AT TIME ZONE 'UTC' to make it immutable
CREATE INDEX idx_zorder ON zorder_test (
    aspiral_zorder_3d(
        EXTRACT(EPOCH FROM (t AT TIME ZONE 'UTC'))::bigint, 
        org_id, 
        user_id
    )
);

ANALYZE zorder_test;

-- Test Query: "Give me all data for 1 hour across ALL organizations"
DO $$
BEGIN
    RAISE NOTICE '--- Testing Composite Index (Scattered) ---';
END $$;

SET enable_indexscan = on;
SET enable_bitmapscan = off;
EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM zorder_test 
WHERE org_id BETWEEN 0 AND 100 
  AND t BETWEEN '2026-04-16 10:00:00' AND '2026-04-16 11:00:00';

DO $$
BEGIN
    RAISE NOTICE '--- Testing Z-Order Index (Clustered) ---';
END $$;

EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM zorder_test 
WHERE aspiral_zorder_3d(EXTRACT(EPOCH FROM (t AT TIME ZONE 'UTC'))::bigint, org_id, user_id) 
      BETWEEN aspiral_zorder_3d(EXTRACT(EPOCH FROM ('2026-04-16 10:00:00'::timestamptz AT TIME ZONE 'UTC'))::bigint, 0, 0)
          AND aspiral_zorder_3d(EXTRACT(EPOCH FROM ('2026-04-16 11:00:00'::timestamptz AT TIME ZONE 'UTC'))::bigint, 100, 100);
