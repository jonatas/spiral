-- Test for Dynamic Tenant Timeline
CREATE TABLE timeline_test (t timestamptz, tenant_id int, val float);

-- Accelerate with an explicit tenant_scale of 100.
-- Under the hood, this should snap to the next power of two (128).
SELECT accelerate('timeline_test', frames => '1h', tenant => ARRAY['tenant_id']);
UPDATE spiral.metadata 
SET columns_metadata = jsonb_set(columns_metadata, '{tenant_scale}', '100')
WHERE view_name = 'timeline_test';

-- Insert a tenant timeline epoch
INSERT INTO spiral.tenants_timeline (table_name, start_t, end_t, tenant_scale, base_offset)
VALUES ('timeline_test', 0, NULL, 128, 0);

SELECT table_name, start_t, tenant_scale FROM spiral.tenants_timeline;

-- Clean up
DROP TABLE timeline_test CASCADE;