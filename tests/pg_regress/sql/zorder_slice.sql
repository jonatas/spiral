LOAD 'spiral';
SET spiral.kickoff_date = '2026-01-01 00:00:00+00';

CREATE TABLE slice_test (
    t timestamptz NOT NULL,
    tenant_id int NOT NULL,
    val double precision
);

-- Accelerate with 1h frames and tenant_id scope
-- Using positional arguments to be safe
SELECT accelerate('slice_test', '1h', ARRAY['tenant_id']);

-- Insert some data
INSERT INTO slice_test (t, tenant_id, val)
SELECT 
    '2026-01-01 00:30:00+00'::timestamptz + (i * interval '1 hour'),
    (i % 10),
    i::double precision
FROM generate_series(0, 99) i;

-- Refresh rollup
SELECT refresh('slice_test');

-- Now query using a box constraint on the Z-order logic
-- Box: (0, 1) to (10800, 2)
-- Using explicit cast to numeric to ensure operator match

SELECT spiral_explain('SELECT SUM(val) FROM slice_test WHERE spiral_zorder(spiral(t), ARRAY[tenant_id]::text[])::numeric <@ ''(0, 1), (10800, 2)''::box');

-- Execute the query
SELECT SUM(val) FROM slice_test 
WHERE spiral_zorder(spiral(t), ARRAY[tenant_id]::text[])::numeric <@ '(0, 1), (10800, 2)'::box;

-- Compare with standard query
SELECT SUM(val) FROM slice_test
WHERE t >= '2026-01-01 00:00:00+00' AND t < '2026-01-01 03:00:00+00'
AND tenant_id BETWEEN 1 AND 2;

DROP TABLE slice_test CASCADE;
