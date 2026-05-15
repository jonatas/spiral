-- Spiral Storage Optimization Benchmark: Time-as-Address
-- Demonstrating minimal storage footprint by removing redundant timestamps.

-- 1. CLEANUP & SETUP
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS storage_bench_delta CASCADE;
CREATE TABLE storage_bench_delta (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision NOT NULL
);

DROP TABLE IF EXISTS main_storage_std;
DROP TABLE IF EXISTS main_storage_zero;

CREATE TABLE main_storage_std (price double precision) USING spiral;
CREATE TABLE main_storage_zero (price double precision) USING spiral;

-- Use 0.1s resolution
SET spiral.minimal_pace = 0.1;
SET spiral.kickoff_date = '2026-04-21';

-- 2. GENERATE DATA (10,000 points per device, 10 devices)
-- Total 100,000 rows
INSERT INTO storage_bench_delta (t, tenant_id, price)
SELECT 
    i / 10, -- t increments every 10 rows (10 devices per 0.1s slot)
    i % 10, -- tenant_id 0-9
    100 + sin(i::float/100)*10 + random()
FROM generate_series(0, 99999) i;

-- 3. PACK DATA
\echo '--- Packing Data (Standard: 64 bytes) ---'
SELECT spiral_pack_delta('storage_bench_delta', 'main_storage_std'::regclass::oid::int);

\echo '--- Packing Data (Zero-Timestamp: 8 bytes) ---'
SELECT spiral_pack_delta_zero('storage_bench_delta', 'main_storage_zero'::regclass::oid::int);

-- 4. STORAGE COMPARISON
\echo '--- Storage Size Comparison ---'
SELECT 'Standard' as type, pg_size_pretty(pg_relation_size('main_storage_std')) as size;
SELECT 'Zero-Timestamp' as type, pg_size_pretty(pg_relation_size('main_storage_zero')) as size;

-- 5. VERIFY TIME RECONSTRUCTION
\echo '--- Verifying Time Reconstruction (0.1s pace) ---'
SELECT 
    t as spiral_coordinate,
    to_timestamptz(t) as reconstructed_time
FROM storage_bench_delta 
WHERE t < 5 AND tenant_id = 0
ORDER BY t;

-- 6. VERIFY READ O(1)
\echo '--- Verifying O(1) Zero-Timestamp Read ---'
SELECT spiral_read_main_zero('main_storage_zero'::regclass::oid::int, 500, 5) as price_at_t500_dev5;
SELECT price FROM storage_bench_delta WHERE t = 500 AND tenant_id = 5;

-- 7. FULL SCAN VALIDATION
\echo '--- Full Reconstruction Scan Comparison ---'
-- Reconstruct all tuples and compare count/avg with original
SELECT 
    COUNT(*) as total_rows,
    round(AVG(value)::numeric, 4) as avg_price
FROM spiral_scan_zero('main_storage_zero'::regclass::oid::int);

SELECT 
    COUNT(*) as total_rows,
    round(AVG(price)::numeric, 4) as avg_price
FROM storage_bench_delta;

-- 8. SCENARIO 2: Different Pace (1s)
\echo '--- Scenario 2: 1s Pace Validation ---'
DROP TABLE IF EXISTS storage_bench_1s CASCADE;
CREATE TABLE storage_bench_1s (t bigint, tenant_id int, price double precision);
DROP TABLE IF EXISTS main_storage_1s;
CREATE TABLE main_storage_1s (price double precision) USING spiral;

SET spiral.minimal_pace = 1.0;
SET spiral.kickoff_date = '2026-01-01';

INSERT INTO storage_bench_1s (t, tenant_id, price)
SELECT 
    spiral('2026-01-01 10:00:00Z'::timestamptz + (i * interval '1 second')),
    (i % 5),
    random() * 50
FROM generate_series(0, 99) i;

SELECT spiral_pack_delta_zero('storage_bench_1s', 'main_storage_1s'::regclass::oid::int);

-- Fetching back and rebuilding
SELECT 
    to_timestamptz(t) as time,
    tenant_id,
    round(value::numeric, 2) as val
FROM spiral_scan_zero('main_storage_1s'::regclass::oid::int)
LIMIT 5;

-- 9. SAFETY CHECK: Pace Mismatch
\echo '--- Safety Check: Pace Mismatch (Expected to fail) ---'
SET spiral.minimal_pace = 0.5;
-- This should fail (return NULL or error) because the relation main_storage_1s was created with pace 1.0
SELECT spiral_read_main_zero('main_storage_1s'::regclass::oid::int, 10, 0);
