LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral CASCADE;
SET client_min_messages = error;

-- kickoff at Unix epoch so t values can be plain integers (seconds since 1970-01-01)
SET spiral.kickoff_date = '1970-01-01 00:00:00Z';

-- ----------------------------------------------------------------
-- MVCC correctness tests for Spiral TAM
-- ----------------------------------------------------------------

DROP TABLE IF EXISTS mvcc_test;
CREATE TABLE mvcc_test (
    t bigint,
    tenant_id int,
    value double precision
) USING spiral;

-- 1. Basic insert then scan
INSERT INTO mvcc_test (t, tenant_id, value) VALUES (100, 0, 999.0);
SELECT value FROM mvcc_test WHERE t = 100 AND tenant_id = 0;

-- 2. Rollback: inserted value must not be visible after ROLLBACK
BEGIN;
INSERT INTO mvcc_test (t, tenant_id, value) VALUES (200, 0, 42.0);
-- Within transaction the value should be visible
SELECT value FROM mvcc_test WHERE t = 200 AND tenant_id = 0;
ROLLBACK;

-- After rollback the value must be gone (0 rows)
SELECT COUNT(*) AS after_rollback_count FROM mvcc_test WHERE t = 200 AND tenant_id = 0;

-- Pre-existing slot (t=100) must be unaffected
SELECT value FROM mvcc_test WHERE t = 100 AND tenant_id = 0;

-- 3. Committed insert must remain visible
BEGIN;
INSERT INTO mvcc_test (t, tenant_id, value) VALUES (300, 0, 77.0);
COMMIT;
SELECT value FROM mvcc_test WHERE t = 300 AND tenant_id = 0;

-- 4. Delete rollback: deleted slot must reappear after ROLLBACK
BEGIN;
DELETE FROM mvcc_test WHERE t = 100 AND tenant_id = 0;
SELECT COUNT(*) AS during_delete FROM mvcc_test WHERE t = 100 AND tenant_id = 0;
ROLLBACK;
SELECT value AS after_delete_rollback FROM mvcc_test WHERE t = 100 AND tenant_id = 0;

-- 5. Committed delete must stay deleted
BEGIN;
DELETE FROM mvcc_test WHERE t = 300 AND tenant_id = 0;
COMMIT;
SELECT COUNT(*) AS after_committed_delete FROM mvcc_test WHERE t = 300 AND tenant_id = 0;

-- Cleanup
DROP TABLE mvcc_test;
