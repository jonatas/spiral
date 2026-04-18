-- Aspiral 3D Benchmark: B-Tree vs. Z-Order vs. GiST
-- Comparing multidimensional indexing strategies for tenants.

CREATE EXTENSION IF NOT EXISTS cube;

DO $$
BEGIN
    RAISE NOTICE '--- Starting Aspiral 3D Multi-Strategy Benchmark ---';
END $$;

-- 1. CLEANUP & SETUP
DROP TABLE IF EXISTS multi_tenant_raw CASCADE;
CREATE TABLE multi_tenant_raw (
    t timestamptz NOT NULL,
    org_id int NOT NULL,
    user_id int NOT NULL,
    val double precision
);

-- Ingest 1M rows
INSERT INTO multi_tenant_raw (t, org_id, user_id, val)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i * interval '1 second'),
    (i % 500) + 1,
    (i % 10000) + 1,
    random() * 100
FROM generate_series(0, 999999) i;

-- 2. CREATE INDEXES
-- Strategy A: Baseline Multi-column B-Tree
CREATE INDEX idx_baseline_3d ON multi_tenant_raw (org_id, user_id, t);

-- Strategy B: 1D Z-Order (Morton Code)
CREATE INDEX idx_aspiral_3d_zorder ON multi_tenant_raw (
    aspiral_zorder_3d(aspiral(t), org_id, user_id)
);

-- Strategy C: 3D GiST (using cube)
CREATE INDEX idx_aspiral_gist_3d ON multi_tenant_raw USING gist (
    cube(ARRAY[aspiral(t)::float, org_id::float, user_id::float])
);

ANALYZE multi_tenant_raw;

-- 3. EXECUTION COMPARISON
DO $$
DECLARE
    start_time timestamptz;
    end_time timestamptz;
    t1 float := aspiral('2026-04-15 00:00:00Z')::float;
    t2 float := aspiral('2026-04-15 05:00:00Z')::float;
    res float;
BEGIN
    -- A. Baseline
    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        SELECT sum(val) INTO res FROM multi_tenant_raw 
        WHERE org_id = 5 AND user_id BETWEEN 1 AND 50 AND t BETWEEN '2026-04-15 00:00:00Z' AND '2026-04-15 05:00:00Z';
    END LOOP;
    RAISE NOTICE 'Baseline (Multi-column B-Tree): %', clock_timestamp() - start_time;

    -- B. Z-Order
    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        SELECT sum(val) INTO res FROM multi_tenant_raw 
        WHERE aspiral_zorder_3d(aspiral(t), org_id, user_id) BETWEEN 
            aspiral_zorder_3d(t1::bigint, 5, 1) AND aspiral_zorder_3d(t2::bigint, 5, 50);
    END LOOP;
    RAISE NOTICE 'Aspiral (1D Z-Order B-Tree): %', clock_timestamp() - start_time;

    -- C. GiST 3D
    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        SELECT sum(val) INTO res FROM multi_tenant_raw 
        WHERE cube(ARRAY[aspiral(t)::float, org_id::float, user_id::float]) 
              @> cube(ARRAY[t1, 5, 1], ARRAY[t2, 5, 50]);
    END LOOP;
    RAISE NOTICE 'Aspiral (3D GiST cube):      %', clock_timestamp() - start_time;
END $$;

-- 4. STORAGE & EXPLAIN
SELECT 
    relname as name,
    pg_size_pretty(pg_relation_size(oid)) as size
FROM pg_class 
WHERE relname IN ('idx_baseline_3d', 'idx_aspiral_3d_zorder', 'idx_aspiral_gist_3d');

SELECT '--- GiST 3D Explain ---' as msg;
EXPLAIN ANALYZE 
SELECT sum(val) FROM multi_tenant_raw 
WHERE cube(ARRAY[aspiral(t)::float, org_id::float, user_id::float]) 
      @> cube(ARRAY[aspiral('2026-04-15 00:00:00Z')::float, 5, 1], 
              ARRAY[aspiral('2026-04-15 05:00:00Z')::float, 5, 50]);
