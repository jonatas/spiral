LOAD 'spiral';
-- Test mathematical integrity of kickoff and spiral(t) mapping
CREATE EXTENSION IF NOT EXISTS spiral CASCADE;

-- 1. Verify spiral(t) is absolute Unix epoch
SELECT spiral('2026-05-27 10:00:00Z'::timestamptz);
-- Should be 1779876000

-- 2. Verify get_kickoff_epoch defaults to 2000-01-01 (946684800)
SELECT spiral_get_storage_stats('pg_class'::regclass::oid::int) -> 'kickoff_epoch';

-- 3. Verify custom kickoff propagates to stats
SET spiral.kickoff_date = '2026-01-01 00:00:00Z';
SELECT spiral_get_storage_stats('pg_class'::regclass::oid::int) -> 'kickoff_epoch';
-- Should be 1767225600

-- 4. Verify reconstruction logic: t = kickoff + t_rel
-- In UI: start_t = kickoff + b.t_range[0]
-- This test simulates the UI logic mathematically
SELECT 1767225600 + (spiral('2026-01-01 00:00:10Z'::timestamptz) - 1767225600) as reconstructed;
-- Should be 1767225610
