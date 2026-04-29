-- 4D Z-Order Benchmark (Time, Org, User, Region)
CREATE EXTENSION IF NOT EXISTS spiral;

DROP TABLE IF EXISTS logs_4d;
CREATE TABLE logs_4d (
    t timestamptz NOT NULL,
    org_id int NOT NULL,
    user_id int NOT NULL,
    region_id int NOT NULL,
    val double precision
);

-- Generate data
INSERT INTO logs_4d (t, org_id, user_id, region_id, val)
SELECT 
    '2026-04-15'::timestamptz + (i * interval '1 minute'),
    (i % 50), 
    ((i / 50) % 50),
    ((i / 2500) % 10),
    random()
FROM generate_series(0, 99999) i;

-- Create 4D Z-Order Index
CREATE INDEX idx_logs_4d_zorder ON logs_4d (
    spiral_zorder(
        EXTRACT(EPOCH FROM (t AT TIME ZONE 'UTC'))::bigint, 
        ARRAY[org_id, user_id, region_id]::integer[]
    )
);

ANALYZE logs_4d;

-- Test Query
DO $$ BEGIN RAISE NOTICE '--- Testing 4D Z-Order Query ---'; END $$;

EXPLAIN (ANALYZE, BUFFERS)
SELECT * FROM logs_4d
WHERE spiral_zorder(EXTRACT(EPOCH FROM (t AT TIME ZONE 'UTC'))::bigint, ARRAY[org_id, user_id, region_id]::integer[])
      BETWEEN spiral_zorder(EXTRACT(EPOCH FROM ('2026-04-16 10:00:00'::timestamptz AT TIME ZONE 'UTC'))::bigint, ARRAY[0, 0, 5]::integer[])
          AND spiral_zorder(EXTRACT(EPOCH FROM ('2026-04-16 11:00:00'::timestamptz AT TIME ZONE 'UTC'))::bigint, ARRAY[50, 50, 5]::integer[]);
