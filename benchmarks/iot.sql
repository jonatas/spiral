-- Comprehensive IoT Storage & Query Benchmark (Side-by-Side Comparison)
SET client_min_messages TO WARNING;
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2025-01-01';

DROP TABLE IF EXISTS delta_iot CASCADE;
CREATE TABLE delta_iot (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision
);

DROP TABLE IF EXISTS standard_storage;
DROP TABLE IF EXISTS compact_storage;
DROP TABLE IF EXISTS blocks_storage;

CREATE TABLE standard_storage (price double precision) USING spiral;
CREATE TABLE compact_storage (price double precision) USING spiral;
CREATE TABLE blocks_storage (price double precision) USING spiral;

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
    std_oid int := 'standard_storage'::regclass::oid;
    comp_oid int := 'compact_storage'::regclass::oid;
    blk_oid int := 'blocks_storage'::regclass::oid;
    dur interval;
BEGIN
    -- 1. Ingestion
    s_t := clock_timestamp(); PERFORM spiral_pack_delta('delta_iot', std_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Standard', 'Ingestion Time', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); PERFORM spiral_pack_delta_compact('delta_iot', comp_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Compact', 'Ingestion Time', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); PERFORM spiral_pack_delta_blocks('delta_iot', blk_oid); dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Ingestion Time', dur::text, extract(epoch from dur));

    -- 2. Random Read (10k)
    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM spiral_read_main(std_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Standard', 'Random Read (10k)', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM spiral_read_main_compact(comp_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Compact', 'Random Read (10k)', dur::text, extract(epoch from dur));

    s_t := clock_timestamp(); FOR i IN 1..10000 LOOP PERFORM spiral_read_main_block_point(blk_oid, (i%1000)::bigint, (i%1000)::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Random Read (10k)', dur::text, extract(epoch from dur));

    -- 3. Range Read (6,400 points)
    s_t := clock_timestamp(); FOR i IN 1..100 LOOP PERFORM spiral_read_main_block_range(blk_oid, i::bigint, 5::bigint); END LOOP; dur := clock_timestamp() - s_t;
    INSERT INTO bench_results VALUES ('Block (XOR)', 'Range Read (Optimized)', dur::text, extract(epoch from dur));
END $$;

-- 4. Storage Sizes
INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Standard', 'Storage Size', pg_relation_size('standard_storage'), pg_size_pretty(pg_relation_size('standard_storage'));

INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Compact', 'Storage Size', pg_relation_size('compact_storage'), pg_size_pretty(pg_relation_size('compact_storage'));

INSERT INTO bench_results (format, metric, val_num, val_text)
SELECT 'Block (XOR)', 'Storage Size', pg_relation_size('blocks_storage'), pg_size_pretty(pg_relation_size('blocks_storage'));

\echo ''
\echo '--- SPIRAL SIDE-BY-SIDE STORAGE COMPARISON ---'
SELECT 
    metric,
    MAX(val_text) FILTER (WHERE format = 'Standard') as "Baseline (Standard)",
    MAX(val_text) FILTER (WHERE format = 'Compact') as "Compact",
    MAX(val_text) FILTER (WHERE format = 'Block (XOR)') as "Spiral Block (XOR)",
    ROUND((MAX(val_num) FILTER (WHERE format = 'Standard') / NULLIF(MAX(val_num) FILTER (WHERE format = 'Block (XOR)'), 0))::numeric, 1) || 'x' as "Improvement"
FROM bench_results
GROUP BY metric
ORDER BY metric DESC;
