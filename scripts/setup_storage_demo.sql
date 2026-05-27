LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral CASCADE;

-- Clean up
DROP TABLE IF EXISTS demo_storage CASCADE;

-- Create a table using Spiral storage
CREATE TABLE demo_storage (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    val float8
) USING spiral WITH (spiral.tenant = 'sensor_id', spiral.cardinality = 'h');

-- Insert some data
INSERT INTO demo_storage (t, sensor_id, val)
SELECT 
    '2026-05-27 10:00:00Z'::timestamptz + (i || ' seconds')::interval,
    (i % 100),
    random() * 100
FROM generate_series(1, 10000) s(i);

-- Force packing to use the XOR compression in storage
SELECT spiral_pack_delta_blocks('demo_storage', 'demo_storage'::regclass::oid::int);

-- Show that it exists
SELECT relname, relkind FROM pg_class WHERE relname = 'demo_storage';
