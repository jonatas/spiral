-- Test Date Anchors (offset columns)
DROP TABLE IF EXISTS sessions CASCADE;
DROP TABLE IF EXISTS events CASCADE;

CREATE TABLE sessions (

    starts_at timestamptz NOT NULL,
    ends_at   timestamptz,
    user_id   int,
    tracked   int
) WITH (spiral.frames = '10m,1h', spiral.tenant = 'user_id');

-- Check physical schema of the base table
-- ends_at should be int4, starts_at should be renamed to t
SELECT attname, atttypid::regtype
FROM pg_attribute
WHERE attrelid = 'sessions'::regclass AND attnum > 0 AND NOT attisdropped
ORDER BY attnum;

-- Check view reconstruction
-- sessions_view should have starts_at and ends_at as timestamptz
SELECT column_name, data_type
FROM information_schema.columns
WHERE table_name = 'sessions_view'
ORDER BY ordinal_position;

-- Insert some data into the physical table
INSERT INTO sessions (t, ends_at, user_id, tracked)
VALUES ('2026-05-15 10:00:00+00', 300, 1, 100);

-- Query via view
SELECT starts_at, ends_at, user_id, tracked FROM sessions_view;

-- Check physical storage
SELECT t, ends_at, user_id, tracked FROM sessions;

-- Test spiral.time_column override
CREATE TABLE events (
    id         serial,
    occurred_at timestamptz NOT NULL,
    resolved_at timestamptz,
    severity    int
) WITH (spiral.frames = '1h,1d', spiral.time_column = 'occurred_at');

-- occurred_at should be renamed to t, resolved_at should be int4
SELECT attname, atttypid::regtype
FROM pg_attribute
WHERE attrelid = 'events'::regclass AND attnum > 0 AND NOT attisdropped
ORDER BY attnum;

-- events_view should have original names
SELECT column_name, data_type
FROM information_schema.columns
WHERE table_name = 'events_view'
ORDER BY ordinal_position;

-- Insert and query events
INSERT INTO events (t, resolved_at, severity)
VALUES ('2026-05-15 11:00:00+00', 1800, 1);

SELECT occurred_at, resolved_at, severity FROM events_view;
SELECT t, resolved_at, severity FROM events;

-- Check rollup view reconstruction
SELECT column_name, data_type
FROM information_schema.columns
WHERE table_name = 'sessions_10m_view'
ORDER BY ordinal_position;
