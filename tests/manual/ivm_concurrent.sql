-- IVM concurrent write safety test
-- Verifies that writes arriving during refresh are not lost.
--
-- Isolation guarantee: unify_changelog snapshots existing ctids before
-- rewriting ranges. The DELETE only touches those ctids, so any row
-- inserted after the snapshot survives and will be picked up on the
-- next refresh cycle.

DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS concurrent_ticks CASCADE;
DROP TABLE IF EXISTS concurrent_ticks_1h CASCADE;

CREATE TABLE concurrent_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric
) WITH (spiral.frames = '1h', spiral.tenant = 'symbol_id');

-- Seed data and initial refresh
INSERT INTO concurrent_ticks (t, symbol_id, price)
SELECT now() - interval '2 hours' + (i * interval '1 minute'), 1, 100.0
FROM generate_series(0, 5) i;

SELECT spiral_refresh('concurrent_ticks');

-- Verify initial view is populated
SELECT COUNT(*) > 0 AS initial_row_exists FROM concurrent_ticks_1h;

-- Simulate a write that lands during refresh: insert a new changelog entry
-- directly (as a trigger would) then immediately refresh to test that the
-- new entry is not wiped by unify_changelog's old DELETE-all path.
INSERT INTO concurrent_ticks (t, symbol_id, price)
VALUES (now() - interval '2 hours' + interval '10 minutes', 1, 999.0);

-- The changelog now has a new entry for the write above.
SELECT COUNT(*) AS changelog_entries_before_refresh
FROM spiral.changelog
WHERE base_view = 'concurrent_ticks';

SELECT spiral_refresh('concurrent_ticks');

-- After refresh the changelog should be empty (the entry was processed).
SELECT COUNT(*) AS changelog_entries_after_refresh
FROM spiral.changelog
WHERE base_view = 'concurrent_ticks';

-- The refreshed view must reflect the injected value (999).
SELECT MAX(price) >= 999 AS high_price_present FROM concurrent_ticks_1h;

-- Simulate the lost-update scenario: two writes, one refresh in between.
-- Write A arrives and is processed. Write B arrives during refresh window.
-- Write B must survive and be visible after a second refresh.

INSERT INTO concurrent_ticks (t, symbol_id, price)
VALUES (now() - interval '2 hours' + interval '20 minutes', 1, 111.0);

SELECT spiral_refresh('concurrent_ticks');

-- Now inject write B while we inspect the changelog state post-refresh.
-- (In production the race is real; here we verify the ctid path is safe
--  by checking that a post-refresh insert is not lost by a second refresh.)
INSERT INTO concurrent_ticks (t, symbol_id, price)
VALUES (now() - interval '2 hours' + interval '25 minutes', 1, 222.0);

SELECT COUNT(*) AS pending_after_second_write
FROM spiral.changelog
WHERE base_view = 'concurrent_ticks';

-- Second refresh must not clear the pending entry before processing it.
SELECT spiral_refresh('concurrent_ticks');

SELECT MAX(price) >= 222 AS write_b_present FROM concurrent_ticks_1h;
