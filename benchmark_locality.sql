-- Aspiral Locality Benchmark: Z-Order, Hilbert Curve, and Multidimensional Clustering
-- Comparing multidimensional indexing strategies for time-series and tenant data.

-- 1. SETUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS locality_test CASCADE;
CREATE TABLE locality_test (
    t timestamptz NOT NULL,
    org_id int NOT NULL,
    user_id int NOT NULL,
    val double precision
);

-- Ingest 1M rows
INSERT INTO locality_test (t, org_id, user_id, val)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i * interval '1 second'),
    (i % 100) + 1,
    (i % 1000) + 1,
    random() * 100
FROM generate_series(0, 999999) i;

-- 2. CREATE INDEXES (Multi-Strategy)
-- Strategy A: Baseline Multi-column B-Tree
CREATE INDEX idx_locality_btree ON locality_test (org_id, user_id, t);

-- Strategy B: 3D Z-Order (Clustered)
CREATE INDEX idx_locality_zorder ON locality_test (
    aspiral_zorder_3d(aspiral(t), org_id, user_id)
);

-- Strategy C: 2D Hilbert Curve (Time interleave with org_id)
CREATE INDEX idx_locality_hilbert ON locality_test (
    aspiral_hilbert_2d((aspiral(t)/3600)::int, org_id)
);

ANALYZE locality_test;

-- 3. EXECUTION COMPARISON
DO $$
DECLARE
    start_time timestamptz;
    t1 float := aspiral('2026-04-15 00:00:00Z'::timestamptz)::float;
    t2 float := aspiral('2026-04-15 05:00:00Z'::timestamptz)::float;
    res float;
BEGIN
    RAISE NOTICE '--- Multidimensional Query: Time + Org + User ---';
    
    -- A. Baseline
    start_time := clock_timestamp();
    FOR i IN 1..50 LOOP
        SELECT sum(val) INTO res FROM locality_test 
        WHERE org_id = 5 AND user_id BETWEEN 1 AND 50 AND t BETWEEN '2026-04-15 00:00:00Z' AND '2026-04-15 05:00:00Z';
    END LOOP;
    RAISE NOTICE 'B-Tree Baseline:     %', clock_timestamp() - start_time;

    -- B. Z-Order
    start_time := clock_timestamp();
    FOR i IN 1..50 LOOP
        SELECT sum(val) INTO res FROM locality_test 
        WHERE aspiral_zorder_3d(aspiral(t), org_id, user_id) BETWEEN 
            aspiral_zorder_3d(t1::bigint, 5, 1) AND aspiral_zorder_3d(t2::bigint, 5, 50);
    END LOOP;
    RAISE NOTICE 'Z-Order (3D):        %', clock_timestamp() - start_time;

    -- C. Hilbert
    start_time := clock_timestamp();
    FOR i IN 1..50 LOOP
        SELECT sum(val) INTO res FROM locality_test 
        WHERE aspiral_hilbert_2d((aspiral(t)/3600)::int, org_id) BETWEEN 
            aspiral_hilbert_2d((t1/3600)::int, 5) AND aspiral_hilbert_2d((t2/3600)::int, 5);
    END LOOP;
    RAISE NOTICE 'Hilbert (2D):        %', clock_timestamp() - start_time;
END $$;

-- 4. STORAGE REPORT
SELECT relname as index_name, pg_size_pretty(pg_relation_size(oid)) as size
FROM pg_class WHERE relname IN ('idx_locality_btree', 'idx_locality_zorder', 'idx_locality_hilbert');
