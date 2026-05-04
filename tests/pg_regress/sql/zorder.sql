LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE multi_tenant (
    t timestamptz NOT NULL,
    tenant_id text NOT NULL,
    sensor_type text NOT NULL,
    val double precision -- Spiral: ohlc
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'tenant_id, sensor_type'
);

-- Check if Z-Order index was created
SELECT relname 
FROM pg_class 
WHERE relname LIKE 'idx_spiral_z_multi_tenant%';

-- Check metadata
SELECT view_name, scope_columns 
FROM spiral.metadata 
WHERE base_view = 'multi_tenant';

-- Check if view index uses Z-Order
-- View indexes for multi-tenant views use spiral_zorder
\d multi_tenant_1m
