-- Aspiral Storage Optimization Benchmark: Time-as-Address
-- Demonstrating minimal storage footprint by removing redundant timestamps.

-- 1. CLEANUP & SETUP
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;

-- Ensure we start from a clean state for binary files
\! rm -rf /tmp/aspiral_main/*.bin

DROP TABLE IF EXISTS storage_bench_delta CASCADE;
CREATE TABLE storage_bench_delta (
    t bigint NOT NULL,
    tenant_id int NOT NULL,
    price double precision NOT NULL
);

-- Use 0.1s resolution
SET aspiral.minimal_pace = 0.1;
SET aspiral.kickoff_date = '2026-04-21';

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
SELECT aspiral_pack_delta('storage_bench_delta', 1001);

\echo '--- Packing Data (Zero-Timestamp: 8 bytes) ---'
SELECT aspiral_pack_delta_zero('storage_bench_delta', 1001);

-- 4. STORAGE COMPARISON
\echo '--- Storage Size Comparison ---'
\! ls -lh /tmp/aspiral_main/1001.bin
\! ls -lh /tmp/aspiral_main/1001_zero.bin

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
SELECT aspiral_read_main_zero(1001, 500, 5) as price_at_t500_dev5;
SELECT price FROM storage_bench_delta WHERE t = 500 AND tenant_id = 5;

-- 7. FULL SCAN VALIDATION
\echo '--- Full Reconstruction Scan Comparison ---'
-- Reconstruct all tuples and compare count/avg with original
SELECT 
    COUNT(*) as total_rows,
    round(AVG(value)::numeric, 4) as avg_price
FROM aspiral_scan_zero(1001);

SELECT 
    COUNT(*) as total_rows,
    round(AVG(price)::numeric, 4) as avg_price
FROM storage_bench_delta;

-- 8. SCENARIO 2: Different Pace (1s)
\echo '--- Scenario 2: 1s Pace Validation ---'
DROP TABLE IF EXISTS storage_bench_1s CASCADE;
CREATE TABLE storage_bench_1s (t bigint, tenant_id int, price double precision);

SET aspiral.minimal_pace = 1.0;
SET aspiral.kickoff_date = '2026-01-01';

INSERT INTO storage_bench_1s (t, tenant_id, price)
SELECT 
    aspiral('2026-01-01 10:00:00Z'::timestamptz + (i * interval '1 second')),
    (i % 5),
    random() * 50
FROM generate_series(0, 99) i;

SELECT aspiral_pack_delta_zero('storage_bench_1s', 1002);

-- Fetching back and rebuilding
SELECT 
    to_timestamptz(t) as time,
    tenant_id,
    round(value::numeric, 2) as val
FROM aspiral_scan_zero(1002)
LIMIT 5;

-- 9. SAFETY CHECK: Pace Mismatch
\echo '--- Safety Check: Pace Mismatch (Expected to fail) ---'
SET aspiral.minimal_pace = 0.5;
-- This should fail (return NULL or error) because the file 1002 was created with pace 1.0
SELECT aspiral_read_main_zero(1002, 10, 0);
