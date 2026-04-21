-- Aspiral Storage Benchmark: Binary Packing and O(1) Access
-- Comparing Standard, Compact, and Block-Compressed formats across Ingestion and Query.

-- 1. SETUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS storage_raw CASCADE;
CREATE TABLE storage_raw (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision
);

-- Ingest 1M rows
INSERT INTO storage_raw (t, tenant_id, price)
SELECT (i / 1000), (i % 1000), random() * 100
FROM generate_series(0, 999999) i;

-- 2. BENCHMARK EXECUTION
DO $$
DECLARE
    start_time timestamptz;
    main_oid int := 777888;
BEGIN
    RAISE NOTICE '--- Ingestion (Packing) Speed (1M Rows) ---';
    
    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta('storage_raw', main_oid);
    RAISE NOTICE 'Standard Packing (64B): %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_compact('storage_raw', main_oid);
    RAISE NOTICE 'Compact Packing (16B):  %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_blocks('storage_raw', main_oid);
    RAISE NOTICE 'Block Packing (XOR):    %', clock_timestamp() - start_time;

    RAISE NOTICE '--- Random Point Read Performance (10k ops) ---';
    
    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Standard (Random): %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main_block_point(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Block Pt (Random): %', clock_timestamp() - start_time;

    RAISE NOTICE '--- Sequential Range Read (Optimized Block vs. Naive) ---';
    
    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        PERFORM aspiral_read_main_block_range(main_oid, i::bigint, 5::bigint);
    END LOOP;
    RAISE NOTICE 'Block Range (Optimized): %', clock_timestamp() - start_time;
END $$;

-- 3. ADAPTIVE LOCALITY TEST
DO $$
BEGIN
    RAISE NOTICE '--- Testing Adaptive Z-Order Scaling ---';
    RAISE NOTICE 'Scale for storage_raw: %', aspiral_zorder_adaptive(0, 'storage_raw', ARRAY['1']);
END $$;

-- 4. STORAGE FOOTPRINT
\! ls -lh /tmp/aspiral_main/777888*.bin
