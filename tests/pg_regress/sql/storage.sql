LOAD 'spiral';
-- Test Time-as-Address storage optimization
CREATE EXTENSION IF NOT EXISTS spiral CASCADE;

-- 1. Setup
SET spiral.kickoff_date = '1970-01-01 00:00:00Z';
SET spiral.minimal_pace = 0.5; -- 500ms resolution

DROP TABLE IF EXISTS delta_storage;
DROP TABLE IF EXISTS main_storage;

CREATE TABLE delta_storage (t bigint, tenant_id int, price double precision);
CREATE TABLE main_storage (t bigint, tenant_id int, price double precision) USING spiral;

-- 2. Populate Delta
INSERT INTO delta_storage (t, tenant_id, price) VALUES
(0, 0, 10.0),
(0, 1, 11.0),
(1, 0, 20.0),
(1, 1, 21.0),
(100, 5, 99.0);

-- 3. Pack from Delta to Main
-- Pass the OID of main_storage
SELECT spiral_pack_delta_zero('delta_storage', 'main_storage'::regclass::oid::int);

-- 4. O(1) Read from Main
SELECT spiral_read_main_zero('main_storage'::regclass::oid::int, 0, 0) as t0_d0;
SELECT spiral_read_main_zero('main_storage'::regclass::oid::int, 1, 1) as t1_d1;
SELECT spiral_read_main_zero('main_storage'::regclass::oid::int, 100, 5) as t100_d5;

-- 5. Scan & Reconstruct Main
SELECT t, tenant_id, value FROM spiral_scan_zero('main_storage'::regclass::oid::int) ORDER BY t, tenant_id;

-- 6. Verify with original
SELECT COUNT(*), AVG(value) FROM spiral_scan_zero('main_storage'::regclass::oid::int);
SELECT COUNT(*), AVG(price) FROM delta_storage;

-- 7. Safety Check: Pace mismatch
SET spiral.minimal_pace = 1.0;
-- This should fail validation (None returned)
SELECT spiral_read_main_zero('main_storage'::regclass::oid::int, 0, 0);

-- 8. Safety Check: OID mismatch
SELECT spiral_read_main_zero('pg_class'::regclass::oid::int, 0, 0);
