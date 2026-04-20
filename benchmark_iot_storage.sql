-- Benchmark: IoT Storage (Standard vs Compact Binary Store)
-- Comparing 64-byte row vs 16-byte compact row storage.

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
    dur_std interval;
    dur_compact interval;
    main_oid int := 123456;
BEGIN
    RAISE NOTICE '--- Starting Binary Packing (1M Rows) ---';
    
    -- 1. Standard Packing (64-byte rows)
    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta('delta_iot', main_oid);
    dur_std := clock_timestamp() - start_time;
    RAISE NOTICE 'Standard Packing (64-byte): %', dur_std;

    -- 2. Compact Packing (16-byte rows)
    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_compact('delta_iot', main_oid);
    dur_compact := clock_timestamp() - start_time;
    RAISE NOTICE 'Compact Packing (16-byte):  %', dur_compact;

    -- 3. Block Packing (2 bytes per point avg)
    start_time := clock_timestamp();
    PERFORM aspiral_pack_delta_blocks('delta_iot', main_oid);
    RAISE NOTICE 'Block Packing (2B/pt avg): %', clock_timestamp() - start_time;

    RAISE NOTICE '--- Testing O(1) Read Performance ---';
    
    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Standard (10k ops): %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main_compact(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Compact (10k ops):  %', clock_timestamp() - start_time;

    start_time := clock_timestamp();
    FOR i IN 1..10000 LOOP
        PERFORM aspiral_read_main_block_point(main_oid, (i % 1000)::bigint, (i % 1000)::bigint);
    END LOOP;
    RAISE NOTICE 'Read Block Pt (10k ops): %', clock_timestamp() - start_time;
END $$;

-- Storage Report (Checking file sizes on disk)
\! ls -lh /tmp/aspiral_main/123456*.bin
