-- Benchmark: IoT Locality (Z-Order vs Hilbert vs B-Tree)
-- Scenario: 10,000 sensors, high-frequency (1 event/sec), 1 hour of data.

DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS iot_readings CASCADE;
CREATE TABLE iot_readings (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    val double precision
);

-- Ingest 1M rows (100 seconds for 10,000 sensors)
INSERT INTO iot_readings (t, sensor_id, val)
SELECT 
    '2026-04-15 00:00:00Z'::timestamptz + (i / 10000 * interval '1 second'),
    (i % 10000),
    random() * 100
FROM generate_series(0, 999999) i;

ANALYZE iot_readings;

-- 1. B-Tree Baseline
CREATE INDEX idx_iot_btree ON iot_readings (sensor_id, t);

-- 2. Standard Z-Order (1h scale)
CREATE INDEX idx_iot_zorder_std ON iot_readings (
    aspiral_zorder(aspiral(t::timestamptz), ARRAY[sensor_id::text])
);

-- 3. Fine-Grained Z-Order (1s scale)
CREATE INDEX idx_iot_zorder_fine ON iot_readings (
    aspiral_zorder_fine(aspiral(t::timestamptz), 1, ARRAY[sensor_id::text])
);

-- 4. Hilbert Curve (1s scale)
CREATE INDEX idx_iot_hilbert ON iot_readings (
    aspiral_hilbert_iot(aspiral(t::timestamptz), 1, sensor_id)
);

ANALYZE iot_readings;

-- Benchmark Queries
DO $$
DECLARE
    start_time timestamptz;
    t1 bigint := aspiral('2026-04-15 00:00:10Z'::timestamptz);
    t2 bigint := aspiral('2026-04-15 00:00:20Z'::timestamptz);
    res float;
BEGIN
    RAISE NOTICE '--- Query: Time Range (10s) across ALL sensors ---';

    -- B-Tree (Should be slow as it's not clustered by time first)
    start_time := clock_timestamp();
    SELECT sum(val) INTO res FROM iot_readings WHERE t BETWEEN '2026-04-15 00:00:10Z' AND '2026-04-15 00:00:20Z';
    RAISE NOTICE 'B-Tree Baseline: %', clock_timestamp() - start_time;

    -- Standard Z-Order (Coarse 1h scale, many false positives in same bucket)
    start_time := clock_timestamp();
    SELECT sum(val) INTO res FROM iot_readings 
    WHERE aspiral_zorder(aspiral(t::timestamptz), ARRAY[sensor_id::text]) 
          BETWEEN aspiral_zorder(t1, ARRAY['0']) AND aspiral_zorder(t2, ARRAY['10000']);
    RAISE NOTICE 'Std Z-Order (1h): %', clock_timestamp() - start_time;

    -- Fine Z-Order (1s scale)
    start_time := clock_timestamp();
    SELECT sum(val) INTO res FROM iot_readings 
    WHERE aspiral_zorder_fine(aspiral(t::timestamptz), 1, ARRAY[sensor_id::text]) 
          BETWEEN aspiral_zorder_fine(t1, 1, ARRAY['0']) AND aspiral_zorder_fine(t2, 1, ARRAY['10000']);
    RAISE NOTICE 'Fine Z-Order (1s): %', clock_timestamp() - start_time;

    -- Hilbert Curve (1s scale)
    start_time := clock_timestamp();
    SELECT sum(val) INTO res FROM iot_readings 
    WHERE aspiral_hilbert_iot(aspiral(t::timestamptz), 1, sensor_id) 
          BETWEEN aspiral_hilbert_iot(t1, 1, 0) AND aspiral_hilbert_iot(t2, 1, 10000);
    RAISE NOTICE 'Hilbert Curve (1s): %', clock_timestamp() - start_time;

END $$;

-- Storage Report
SELECT 
    relname as index_name,
    pg_size_pretty(pg_relation_size(oid)) as size
FROM pg_class 
WHERE relname IN ('idx_iot_btree', 'idx_iot_zorder_std', 'idx_iot_zorder_fine', 'idx_iot_hilbert');
