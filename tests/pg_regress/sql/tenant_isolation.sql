LOAD 'spiral';
-- Test for tenant isolation and surgical healing
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE multi_tenant_test (
    t timestamptz NOT NULL,
    tenant_id text NOT NULL,
    val double precision -- Spiral: sum
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'tenant_id'
);

-- Ingest data for two tenants
INSERT INTO multi_tenant_test (t, tenant_id, val) VALUES
('2026-04-15 10:00:05Z', 'A', 10.0),
('2026-04-15 10:00:05Z', 'B', 20.0);

-- Initial refresh
SELECT spiral_refresh('multi_tenant_test');

-- Verify both tenants are present
SELECT t, tenant_id, val FROM multi_tenant_test_1m ORDER BY tenant_id;

-- Now update only Tenant A
UPDATE multi_tenant_test SET val = 15.0 WHERE t = '2026-04-15 10:00:05Z' AND tenant_id = 'A';

-- Check changelog - should ONLY have Tenant A
SELECT tenant_id, scope_values, to_timestamptz(t_start) 
FROM spiral.changelog c,
     LATERAL (SELECT v FROM jsonb_each_text(c.scope_values) WHERE key = 'tenant_id') as t(tenant_id);

-- Explain query for Tenant B - should be FULLY accelerated (no RAW fallback)
-- because Tenant B is clean.
SELECT spiral_explain('SELECT sum(val) FROM multi_tenant_test WHERE tenant_id = ''B'' AND t >= ''2026-04-15 10:00:00Z'' AND t < ''2026-04-15 10:01:00Z''');

-- Explain query for Tenant A - should show RAW fallback
SELECT spiral_explain('SELECT sum(val) FROM multi_tenant_test WHERE tenant_id = ''A'' AND t >= ''2026-04-15 10:00:00Z'' AND t < ''2026-04-15 10:01:00Z''');

-- Refresh for Tenant A only
SELECT spiral_refresh('multi_tenant_test', 'tenant_id = ''A''');

-- Check changelog - Tenant A should be gone, but if there were others they should remain.
-- (In this test, only A was dirty, so it should be empty now)
SELECT count(*) FROM spiral.changelog;

-- Verify final data
SELECT t, tenant_id, val FROM multi_tenant_test_1m ORDER BY tenant_id;
