LOAD 'spiral';
-- Test that regular tables are NOT affected by Spiral
CREATE EXTENSION IF NOT EXISTS spiral;
SET client_min_messages = info;

DROP TABLE IF EXISTS regular_table;
DROP TABLE IF EXISTS regular_table_with_magic;
DROP TABLE IF EXISTS spiral_table CASCADE;

-- 1. Create a regular table without WITH (spiral...) or magic header
CREATE TABLE regular_table (
    t timestamptz NOT NULL,
    val double precision
);

-- 2. Verify NO metadata was created for it
SELECT count(*) FROM spiral.metadata WHERE base_view = 'regular_table' OR view_name = 'regular_table';

-- 3. Verify NO triggers were created for it
SELECT tgname FROM pg_trigger WHERE tgrelid = 'regular_table'::regclass AND tgname LIKE 'spiral%';

-- 4. Verify NO related views were created
SELECT relname FROM pg_class WHERE relname LIKE 'regular_table%';

-- 5. Create a table with column magic comments but NO top-level -- Spiral: enabled
CREATE TABLE regular_table_with_magic (
    t timestamptz NOT NULL,
    val double precision -- Spiral: ohlc
);

-- 6. Verify NO metadata, triggers or views
SELECT count(*) FROM spiral.metadata WHERE base_view = 'regular_table_with_magic';
SELECT tgname FROM pg_trigger WHERE tgrelid = 'regular_table_with_magic'::regclass AND tgname LIKE 'spiral%';
SELECT relname FROM pg_class WHERE relname LIKE 'regular_table_with_magic%';

-- 7. Create a table WITH top-level -- Spiral: enabled
-- spiral: enabled
CREATE TABLE spiral_table (
    t timestamptz NOT NULL,
    val double precision -- Spiral: sum
);

-- 8. Verify metadata and triggers exist for this one
SELECT count(*) FROM spiral.metadata WHERE base_view = 'spiral_table';
SELECT count(*) > 0 FROM pg_trigger WHERE tgrelid = 'spiral_table'::regclass AND tgname LIKE 'spiral%';
SELECT relname FROM pg_class WHERE relname LIKE 'spiral_table_ohlcv%';

-- 9. Test on-demand acceleration for regular tables
DROP TABLE IF EXISTS standard_metrics CASCADE;
DROP TABLE IF EXISTS standard_metrics_1m CASCADE;
DROP TABLE IF EXISTS standard_metrics_1h CASCADE;
DELETE FROM spiral.metadata WHERE base_view = 'standard_metrics';
DELETE FROM spiral.sources WHERE base_view = 'standard_metrics';

CREATE TABLE standard_metrics (
    t timestamptz NOT NULL,
    val double precision
);

INSERT INTO standard_metrics (t, val)
SELECT '2026-05-25 10:00:00Z'::timestamptz + (n || ' seconds')::interval, 10.0
FROM generate_series(1, 100) n;

-- Register for acceleration
SELECT accelerate('standard_metrics', frames => '1m,1h', columns => ARRAY['val sum', 'val stats']);

-- Verify rollups created
SELECT count(*) FROM spiral.metadata WHERE base_view = 'standard_metrics' AND frame_seconds > 0;

-- Populate rollups
SELECT spiral_refresh('standard_metrics');

-- Verify acceleration (SUM)
EXPLAIN (COSTS OFF) SELECT sum(val) FROM standard_metrics WHERE t >= '2026-05-25 10:00:00Z' AND t < '2026-05-25 11:00:00Z';
SELECT sum(val) FROM standard_metrics WHERE t >= '2026-05-25 10:00:00Z' AND t < '2026-05-25 11:00:00Z';

-- Verify acceleration (STATS)
EXPLAIN (COSTS OFF) SELECT spiral_stats(val) FROM standard_metrics WHERE t >= '2026-05-25 10:00:00Z' AND t < '2026-05-25 11:00:00Z';
SELECT spiral_stats_mean(spiral_stats(val)) FROM standard_metrics WHERE t >= '2026-05-25 10:00:00Z' AND t < '2026-05-25 11:00:00Z';
