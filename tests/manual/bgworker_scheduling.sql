-- bgworker_scheduling.sql: validate scope-affinity scheduling under multi-tenant write load.
--
-- Strategy: create N tables with a tenant (scope) column. Insert rows for M distinct
-- tenants. With max_workers > 1, scopes should be processed in parallel — each worker
-- claims disjoint (base_view, scope_values) pairs via advisory locks.
--
-- Observability: spiral.scope_status shows per-scope lag before and after workers drain.

DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- Enable parallel workers
ALTER SYSTEM SET spiral.worker_enabled = true;
ALTER SYSTEM SET spiral.max_workers = 4;
ALTER SYSTEM SET spiral.worker_batch_size = 5;
SELECT pg_reload_conf();
SELECT pg_sleep(1);

-- -------------------------------------------------------------------------
-- Setup: 1 table, 10 tenants (scopes)
-- -------------------------------------------------------------------------
DROP TABLE IF EXISTS sched_events CASCADE;
CREATE TABLE sched_events (
    t        timestamptz NOT NULL,
    tenant   int         NOT NULL,
    val      numeric
) WITH (spiral.frames = '1m', spiral.tenant = 'tenant');

-- Insert 10 rows for 10 distinct tenants, spread across 3 time buckets
INSERT INTO sched_events
SELECT
    '2026-05-01 10:00:00Z'::timestamptz + (i % 3) * interval '1 minute',
    i,   -- tenant id = scope
    random() * 100
FROM generate_series(1, 10) AS i;

-- Verify all 10 scopes are dirty before any refresh
SELECT count(*) = 10 AS ten_dirty_scopes
FROM spiral.scope_status
WHERE base_view = 'sched_events';

-- Verify lag is positive for all dirty scopes
SELECT bool_and(lag > interval '0') AS all_scopes_have_lag
FROM spiral.scope_status
WHERE base_view = 'sched_events';

-- -------------------------------------------------------------------------
-- After workers drain (sleep to give them time)
-- -------------------------------------------------------------------------
SELECT pg_sleep(5);

-- All scopes should be clean — zero dirty entries
SELECT count(*) = 0 AS fully_drained
FROM spiral.scope_status
WHERE base_view = 'sched_events';

-- Rollup rows: 10 tenants × 3 time buckets = 30 rows expected
SELECT count(*) = 30 AS expected_rollup_count FROM sched_events_1m;

-- -------------------------------------------------------------------------
-- Sustained write: add more rows for a subset of tenants
-- -------------------------------------------------------------------------
INSERT INTO sched_events
SELECT
    '2026-05-01 10:10:00Z'::timestamptz + (i % 5) * interval '1 minute',
    i,
    random() * 100
FROM generate_series(1, 5) AS i;

-- 5 scopes now dirty again
SELECT count(*) = 5 AS five_dirty_scopes
FROM spiral.scope_status
WHERE base_view = 'sched_events';

-- Workers drain again
SELECT pg_sleep(5);

SELECT count(*) = 0 AS fully_drained_again
FROM spiral.scope_status
WHERE base_view = 'sched_events';

-- -------------------------------------------------------------------------
-- Verify worker processes are running
-- -------------------------------------------------------------------------
SELECT count(*) > 0 AS workers_active
FROM pg_stat_activity
WHERE backend_type LIKE 'Spiral Worker%';

-- -------------------------------------------------------------------------
-- Cleanup
-- -------------------------------------------------------------------------
DROP TABLE sched_events CASCADE;
ALTER SYSTEM RESET spiral.max_workers;
ALTER SYSTEM RESET spiral.worker_batch_size;
SELECT pg_reload_conf();
