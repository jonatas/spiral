-- Test Transparent timestamptz reconstruction
CREATE EXTENSION IF NOT EXISTS spiral;
DROP TABLE IF EXISTS sessions CASCADE;

CREATE TABLE sessions (
    t        timestamptz NOT NULL,
    ends_at  timestamptz,           -- Spiral: range_merge
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

-- Actual value reconstructed correctly
-- Note: we use 10m frame, so 10:00 to 10:10 bucket. 
-- Max of (10:08-10:00)=480s and (10:12-10:03)=540s is 540s.
-- Wait, actually they are in the same 10m bucket starting at 10:00.
-- ends_at for first row is 10:08 (offset 480 from 10:00)
-- ends_at for second row is 10:12 (offset 720 from 10:00)
-- Wait, 10:12 is offset 720 from 10:00.
-- So max(480, 720) = 720. 10:00 + 720s = 10:12.
SELECT t, ends_at, duration FROM sessions_10m WHERE user_id = 1;

-- Acceleration + reconstruction: both active when time filter given
SELECT t, ends_at FROM sessions_10m
WHERE t >= '2026-01-01 10:00:00+00' AND t < '2026-01-01 11:00:00+00'
  AND user_id = 1;

DROP TABLE sessions CASCADE;
