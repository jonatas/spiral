-- Test that regular tables are NOT affected by Aspiral
CREATE EXTENSION IF NOT EXISTS aspiral;
SET client_min_messages = info;

DROP TABLE IF EXISTS regular_table;
DROP TABLE IF EXISTS regular_table_with_magic;
DROP TABLE IF EXISTS aspiral_table CASCADE;

-- 1. Create a regular table without WITH (aspiral...) or magic header
CREATE TABLE regular_table (
    t timestamptz NOT NULL,
    val double precision
);

-- 2. Verify NO metadata was created for it
SELECT count(*) FROM aspiral.metadata WHERE base_view = 'regular_table' OR view_name = 'regular_table';

-- 3. Verify NO triggers were created for it
SELECT tgname FROM pg_trigger WHERE tgrelid = 'regular_table'::regclass AND tgname LIKE 'aspiral%';

-- 4. Verify NO related views were created
SELECT relname FROM pg_class WHERE relname LIKE 'regular_table%';

-- 5. Create a table with column magic comments but NO top-level -- Aspiral: enabled
CREATE TABLE regular_table_with_magic (
    t timestamptz NOT NULL,
    val double precision -- Aspiral: ohlc
);

-- 6. Verify NO metadata, triggers or views
SELECT count(*) FROM aspiral.metadata WHERE base_view = 'regular_table_with_magic';
SELECT tgname FROM pg_trigger WHERE tgrelid = 'regular_table_with_magic'::regclass AND tgname LIKE 'aspiral%';
SELECT relname FROM pg_class WHERE relname LIKE 'regular_table_with_magic%';

-- 7. Create a table WITH top-level -- Aspiral: enabled
-- aspiral: enabled
CREATE TABLE aspiral_table (
    t timestamptz NOT NULL,
    val double precision -- Aspiral: sum
);

-- 8. Verify metadata and triggers exist for this one
SELECT count(*) FROM aspiral.metadata WHERE base_view = 'aspiral_table';
SELECT count(*) > 0 FROM pg_trigger WHERE tgrelid = 'aspiral_table'::regclass AND tgname LIKE 'aspiral%';
SELECT relname FROM pg_class WHERE relname LIKE 'aspiral_table_ohlcv%';
