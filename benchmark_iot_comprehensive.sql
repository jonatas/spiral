-- Comprehensive IoT Storage & Query Benchmark
-- Comparing Standard, Compact, and Block formats across Ingestion, Storage, and Query (Sequential vs Random).

DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS delta_iot CASCADE;
CREATE TABLE delta_iot (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision
);

-- 1M rows of IoT data
INSERT INTO delta_iot (t, tenant_id, price)
SELECT 
    (i / 1000), -- time in seconds
    (i % 1000), -- 1000 tenants
    random() * 100
FROM generate_series(0, 999999) i;

DO $$
DECLARE
    start_time timestamptz;
    main_oid int := 987654;
    res float;
    res_arr float[];
BEGIN
    RAISE NOTICE '========== 1. INGESTION (PACKING) SPEED ==========';
    
    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta('delta_iot', main_oid);
    RAISE NOTICE 'Standard Packing (64-byte): %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_compact('delta_iot', main_oid);
    RAISE NOTICE 'Compact Packing (16-byte):  %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_blocks('delta_iot', main_oid);
    RAISE NOTICE 'Block Packing (XOR):        %', clock_timestamp() - start_time;

    RAISE NOTICE '';
    RAISE NOTICE '========== 2. RANDOM SINGLE-POINT READ (CPU Overhead) ==========';
    -- 10,000 random point reads.
    
    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Standard (Random): %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main_compact(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Compact (Random):  %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main_block_point(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Block Pt (Random): % (Includes XOR Loop overhead)', clock_timestamp() - start_time;

    RAISE NOTICE '';
    RAISE NOTICE '========== 3. SEQUENTIAL RANGE READ (64 Points) ==========';
    -- Fetching 64 points for a single sensor (one block).
    
    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        FOR t_step IN 0..63 LOOP
            PERFORM aspiral_read_main_compact(main_oid, (i * 64 + t_step)::bigint, 5::bigint);
        END LOOP;
    END LOOP;
    RAISE NOTICE 'Compact Range (Naive Loop): % (6,400 seeks)', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        FOR t_step IN 0..63 LOOP
            PERFORM aspiral_read_main_block_point(main_oid, (i * 64 + t_step)::bigint, 5::bigint);
        END LOOP;
    END LOOP;
    RAISE NOTICE 'Block Range (Naive Loop):   % (6,400 seeks + O(N^2) XOR overhead)', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..100 LOOP
        PERFORM aspiral_read_main_block_range(main_oid, i::bigint, 5::bigint);
    END LOOP;
    RAISE NOTICE 'Block Range (Optimized):   % (100 seeks + O(N) sequential XOR)', clock_timestamp() - start_time;

END $$;

RAISE NOTICE '';
RAISE NOTICE '========== 4. STORAGE FOOTPRINT ==========';
\! ls -lh /tmp/aspiral_main/987654*.bin
