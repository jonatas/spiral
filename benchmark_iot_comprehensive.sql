-- Comprehensive IoT Storage & Query Benchmark (Side-by-Side Comparison)
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2025-01-01';

DROP TABLE IF EXISTS delta_iot CASCADE;
CREATE TABLE delta_iot (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision
);

-- Results tracking table
CREATE TEMP TABLE bench_results (
    format TEXT,
    metric TEXT,
    val_text TEXT,
    val_num FLOAT
);

-- 1M rows of IoT data
\echo '--- [STAGE 1] Ingesting 1M rows for storage testing ---'
INSERT INTO delta_iot (t, tenant_id, price)
SELECT (i / 1000), (i % 1000), random() * 100 FROM generate_series(0, 999999) i;

DO $$
DECLARE
    s_t timestamptz;
    main_oid int := 987654;
    dur interval;
BEGIN
    -- 1. Ingestion
    s_t := clock_timestamp(); PERFORM aspiral_pack_delta('delta_iot', main_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Standard', 'Ingestion Time', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); PERFORM aspiral_pack_delta_compact('delta_iot', main_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Compact', 'Ingestion Time', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); PERFORM aspiral_pack_delta_blocks('delta_iot', main_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Ingestion Time', dur::text, extract(epoch from dur));

    -- 2. Random Read (10k)
    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM aspiral_read_main(main_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Standard', 'Random Read (10k)', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM aspiral_read_main_compact(main_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Compact', 'Random Read (10k)', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM aspiral_read_main_block_point(main_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Random Read (10k)', dur::text, extract(epoch from dur));

    -- 3. Range Read (6,400 points)
    s_t := clock_timestamp(); FOR i IN 1..100 LOOP PERFORM aspiral_read_main_block_range(main_oid, i::bigint, 5::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Range Read (Optimized)', dur::text, extract(epoch from dur));
END $$;

-- 4. Storage Sizes
INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Standard', 'Storage Size', pg_size_bytes(size), size FROM (SELECT pg_ls_dir('/tmp/aspiral_main/') as name) s, LATERAL (SELECT (pg_stat_file('/tmp/aspiral_main/' || name)).size::text as size WHERE name = '987654.bin') f;

INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Compact', 'Storage Size', pg_size_bytes(size), size FROM (SELECT pg_ls_dir('/tmp/aspiral_main/') as name) s, LATERAL (SELECT (pg_stat_file('/tmp/aspiral_main/' || name)).size::text as size WHERE name = '987654_compact.bin') f;

INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Block (XOR)', 'Storage Size', pg_size_bytes(size), size FROM (SELECT pg_ls_dir('/tmp/aspiral_main/') as name) s, LATERAL (SELECT (pg_stat_file('/tmp/aspiral_main/' || name)).size::text as size WHERE name = '987654_blocks.bin') f;

\echo ''
\echo '--- ASPIRAL SIDE-BY-SIDE STORAGE COMPARISON ---'
SELECT 
    metric,
    MAX(val_text) FILTER (WHERE format = 'Standard') as "Baseline (Standard)",
    MAX(val_text) FILTER (WHERE format = 'Compact') as "Compact",
    MAX(val_text) FILTER (WHERE format = 'Block (XOR)') as "Aspiral Block (XOR)",
    ROUND((MAX(val_num) FILTER (WHERE format = 'Standard') / NULLIF(MAX(val_num) FILTER (WHERE format = 'Block (XOR)'), 0))::numeric, 1) || 'x' as "Improvement"
FROM bench_results
GROUP BY metric
ORDER BY metric DESC;
