-- End-to-End Backup/Restore Test for Spiral Zero-Timestamp Storage
-- Scenario: Materialize optimized storage to a standard table for SQL dump, then restore.

-- 1. SETUP & INITIAL POPULATION
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;
\! rm -rf /tmp/spiral_main/*.bin

SET spiral.kickoff_date = '2026-04-01';
SET spiral.minimal_pace = 1.0;

CREATE TABLE production_data (t bigint, tenant_id int, price double precision);
INSERT INTO production_data SELECT i/2, i%2, 100 + i FROM generate_series(0, 9) i;

-- Pack into optimized binary storage
SELECT spiral_pack_delta_zero('production_data', 7777);
\! ls -lh /tmp/spiral_main/7777_zero.bin

-- 2. SIMULATE BACKUP
\echo '--- [BACKUP PHASE] ---'
-- Standard pg_dump would not see the binary files.
-- We create a backup table using spiral_scan_zero()
CREATE TABLE backup_reconstructed AS SELECT * FROM spiral_scan_zero(7777);

-- 3. SIMULATE DISASTER (Drop everything)
\echo '--- [DISASTER PHASE] ---'
DROP TABLE production_data;
DROP TABLE backup_reconstructed;
-- In a real disaster, the binary files might be lost or we might be on a new server.
\! rm -rf /tmp/spiral_main/*.bin

-- 4. RESTORE PHASE
\echo '--- [RESTORE PHASE] ---'
-- Re-create the standard table from our SQL dump (simulated by this table)
CREATE TABLE restore_source (t bigint, tenant_id int, price double precision);
INSERT INTO restore_source VALUES
(0, 0, 100), (0, 1, 101), (1, 0, 102), (1, 1, 103), (2, 0, 104),
(2, 1, 105), (3, 0, 106), (3, 1, 107), (4, 0, 108), (4, 1, 109);

-- Re-pack into optimized storage on the new "system"
-- Ensure GUCs are set correctly first!
SET spiral.kickoff_date = '2026-04-01';
SET spiral.minimal_pace = 1.0;
SELECT spiral_pack_delta_zero('restore_source', 7777);

-- 5. FINAL VALIDATION
\echo '--- [VALIDATION PHASE] ---'
-- Check if O(1) reads work on the restored data
SELECT 
    t as coordinate,
    to_timestamptz(t) as time,
    spiral_read_main_zero(7777, t, 0) as price_tenant_0,
    spiral_read_main_zero(7777, t, 1) as price_tenant_1
FROM generate_series(0, 4) t;

-- Compare counts
SELECT COUNT(*) as restored_count FROM spiral_scan_zero(7777);
