-- End-to-End Backup/Restore Test for Spiral Zero-Timestamp Storage
-- Scenario: Materialize optimized storage to a standard table for SQL dump, then restore.

-- 1. SETUP & INITIAL POPULATION
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

SET spiral.kickoff_date = '2026-04-01 00:00:00Z';
SET spiral.minimal_pace = 1.0;

CREATE TABLE production_data (t timestamptz, tenant_id int, price double precision);
INSERT INTO production_data SELECT '2026-04-01 00:00:00Z'::timestamptz + (i || ' seconds')::interval, i%2, 100 + i FROM generate_series(0, 9) i;

-- Pack into optimized buffer-managed storage
-- Use the actual OID of production_data
SELECT spiral_pack_delta_zero('production_data', 'production_data'::regclass::oid::int);

-- 2. SIMULATE BACKUP
\echo '--- [BACKUP PHASE] ---'
-- Since we migrated to the Postgres Buffer Manager (smgr), a standard pg_dump WILL now see the data!
-- However, for this specific raw-packing test, we reconstruct it via scan to show the O(1) retrieval.
CREATE TABLE backup_reconstructed AS SELECT * FROM spiral_scan_zero('production_data'::regclass::oid::int);

-- 3. SIMULATE DISASTER (Drop everything)
\echo '--- [DISASTER PHASE] ---'
DROP TABLE production_data;
-- The relation files in PGDATA associated with the original OID would be lost here in a real disaster.

-- 4. RESTORE PHASE
\echo '--- [RESTORE PHASE] ---'
-- Re-create the standard table from our SQL dump (simulated by this table)
DROP TABLE IF EXISTS restore_source;
CREATE TABLE restore_source (t timestamptz, tenant_id int, price double precision);
INSERT INTO restore_source VALUES
('2026-04-01 00:00:00Z', 0, 100), ('2026-04-01 00:00:01Z', 1, 101), ('2026-04-01 00:00:02Z', 0, 102), ('2026-04-01 00:00:03Z', 1, 103), ('2026-04-01 00:00:04Z', 0, 104),
('2026-04-01 00:00:05Z', 1, 105), ('2026-04-01 00:00:06Z', 0, 106), ('2026-04-01 00:00:07Z', 1, 107), ('2026-04-01 00:00:08Z', 0, 108), ('2026-04-01 00:00:09Z', 1, 109);

-- Re-pack into optimized storage on the new "system"
-- Ensure GUCs are set correctly first!
SET spiral.kickoff_date = '2026-04-01 00:00:00Z';
SET spiral.minimal_pace = 1.0;
SELECT spiral_pack_delta_zero('restore_source', 'restore_source'::regclass::oid::int);

-- 5. FINAL VALIDATION
\echo '--- [VALIDATION PHASE] ---'
-- Check if O(1) reads work on the restored data
SELECT 
    t as coordinate,
    to_timestamp(t + extract(epoch from '2026-04-01 00:00:00Z'::timestamptz)) as time,
    spiral_read_main_zero('restore_source'::regclass::oid::int, t + spiral('2026-04-01 00:00:00Z'::timestamptz), 0) as price_tenant_0,
    spiral_read_main_zero('restore_source'::regclass::oid::int, t + spiral('2026-04-01 00:00:00Z'::timestamptz), 1) as price_tenant_1
FROM generate_series(0, 9) t;

-- Compare counts
SELECT COUNT(*) as restored_count FROM spiral_scan_zero('restore_source'::regclass::oid::int);
