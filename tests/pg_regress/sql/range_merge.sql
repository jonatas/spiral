-- Test range_max_end (span semantics): stores max(ends_at) - bucket_start as int4 offset.
-- "range_merge" accepted as backward-compat alias.
-- NOTE: span semantics ≠ union semantics. Disjoint sessions in the same bucket report
-- the span from bucket_start to max(ends_at), NOT the sum of covered intervals.
CREATE EXTENSION IF NOT EXISTS spiral;
DROP TABLE IF EXISTS sessions CASCADE;

CREATE TABLE sessions (
    t        timestamptz NOT NULL,
    ends_at  timestamptz,           -- Spiral: range_max_end
    user_id  int,
    duration int                    -- Spiral: sum
) WITH (spiral.frames = '10m,1h', spiral.tenant = 'user_id');

-- Physical storage: ends_at int4 in rollup, timestamptz in base
SELECT attname, atttypid::regtype FROM pg_attribute
WHERE attrelid = 'sessions_10m'::regclass AND attnum > 0 AND NOT attisdropped
ORDER BY attnum;
-- ends_at should be integer

SELECT attname, atttypid::regtype FROM pg_attribute
WHERE attrelid = 'sessions'::regclass AND attnum > 0 AND NOT attisdropped
ORDER BY attnum;
-- ends_at should be timestamp with time zone

-- Insert with real timestamps
INSERT INTO sessions VALUES
    ('2026-01-01 10:00:00+00', '2026-01-01 10:08:00+00', 1, 480),
    ('2026-01-01 10:03:00+00', '2026-01-01 10:12:00+00', 1, 540);

SELECT spiral_refresh('sessions');

-- Hook reconstructs: pg_typeof should show timestamptz, not integer
SELECT pg_typeof(ends_at) FROM sessions_10m LIMIT 1;
-- expect: timestamp with time zone

-- Span semantics: max(ends_at) across both rows is 10:12 (offset 720s from 10:00).
-- Both rows land in the same 10m bucket starting at 10:00.
SELECT t, ends_at, duration FROM sessions_10m WHERE user_id = 1;

-- Acceleration + reconstruction: both active when time filter given
SELECT t, ends_at FROM sessions_10m
WHERE t >= '2026-01-01 10:00:00+00' AND t < '2026-01-01 11:00:00+00'
  AND user_id = 1;

-- Disjoint session test: span ≠ union.
-- Session A: 10:00-10:05 (300s), Session B: 10:07-10:09 (120s).
-- True union = 420s. Span = max(10:09) - 10:00 = 540s.
-- range_max_end reports SPAN (540s), not union (420s).
DROP TABLE sessions CASCADE;
CREATE TABLE sessions (
    t        timestamptz NOT NULL,
    ends_at  timestamptz,           -- Spiral: range_max_end
    user_id  int
) WITH (spiral.frames = '10m', spiral.tenant = 'user_id');

INSERT INTO sessions VALUES
    ('2026-01-01 10:00:00+00', '2026-01-01 10:05:00+00', 1),
    ('2026-01-01 10:07:00+00', '2026-01-01 10:09:00+00', 1);

SELECT spiral_refresh('sessions');

-- ends_at reconstructed = 10:00 + 540s = 10:09:00 (span, not 10:07:00 union end)
SELECT t, ends_at FROM sessions_10m WHERE user_id = 1;

DROP TABLE sessions CASCADE;
