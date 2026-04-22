-- Test Time-as-Address storage optimization
CREATE EXTENSION IF NOT EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;

-- Clean up binary files from previous runs
\! rm -rf /tmp/aspiral_main/*.bin

-- 1. Setup
SET aspiral.kickoff_date = '2026-04-15';
SET aspiral.minimal_pace = 0.5; -- 500ms resolution

CREATE TABLE test_storage (t bigint, tenant_id int, price double precision);

-- 2. Populate
INSERT INTO test_storage (t, tenant_id, price) VALUES
(0, 0, 10.0),
(0, 1, 11.0),
(1, 0, 20.0),
(1, 1, 21.0),
(100, 5, 99.0);

-- 3. Pack
SELECT aspiral_pack_delta_zero('test_storage', 9999);

-- 4. O(1) Read
SELECT aspiral_read_main_zero(9999, 0, 0) as t0_d0;
SELECT aspiral_read_main_zero(9999, 1, 1) as t1_d1;
SELECT aspiral_read_main_zero(9999, 100, 5) as t100_d5;

-- 5. Scan & Reconstruct
SELECT t, tenant_id, value FROM aspiral_scan_zero(9999) ORDER BY t, tenant_id;

-- 6. Verify with original
SELECT COUNT(*), AVG(value) FROM aspiral_scan_zero(9999);
SELECT COUNT(*), AVG(price) FROM test_storage;

-- 7. Safety Check: Pace mismatch
SET aspiral.minimal_pace = 1.0;
-- This should fail validation
SELECT aspiral_read_main_zero(9999, 0, 0);

-- 8. Safety Check: OID mismatch
-- Try to read OID 9999 data using OID 8888
SELECT aspiral_read_main_zero(8888, 0, 0);

-- 9. Sanity bounds check
SET aspiral.minimal_pace = 0.5;
-- Point very far in the future
INSERT INTO test_storage (t, tenant_id, price) VALUES (9999999999, 0, 0.0);
-- Should emit warning and skip
SELECT aspiral_pack_delta_zero('test_storage', 9999);

-- Cleanup
DROP TABLE test_storage;
