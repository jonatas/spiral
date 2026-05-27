-- Force drop everything related to spiral to start clean
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

-- Create iot_metrics with Spiral storage
CREATE TABLE iot_metrics (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    value double precision
) USING spiral WITH (spiral.tenant = 'sensor_id', spiral.cardinality = 'h');

-- Insert 1000 rows - keeping it small first to confirm it works
INSERT INTO iot_metrics (t, sensor_id, value)
SELECT 
    '2026-05-27 00:00:00Z'::timestamptz + (i || ' seconds')::interval,
    (i % 128),
    random() * 100
FROM generate_series(1, 1000) s(i);

-- Register it in metadata so the dashboard sees it
-- The UI depends on spiral.metadata to show the hierarchies
-- Standard 'sensor_data' script already does this, but for 'iot_metrics'
-- using Custom Storage, we need to ensure it's tracked.
-- Actually, the utility hook should have caught the CREATE TABLE above.

-- Pack blocks
SELECT spiral_pack_delta_blocks('iot_metrics', 'iot_metrics'::regclass::oid::int);
