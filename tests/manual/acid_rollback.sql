DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS acid_ticks CASCADE;
DROP TABLE IF EXISTS acid_ticks_1h CASCADE;

CREATE TABLE acid_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric -- Spiral: sum
) WITH (spiral.frames = '1h', spiral.tenant = 'symbol_id');

-- Insert initial data
INSERT INTO acid_ticks (t, symbol_id, price)
VALUES ('2026-04-15 10:05:00Z', 42, 100);

-- Refresh the view
SELECT spiral_refresh('acid_ticks_1h');

-- Verify initial state
SELECT price FROM acid_ticks_1h WHERE symbol_id = 42;

-- Start a transaction that fails
BEGIN;
INSERT INTO acid_ticks (t, symbol_id, price)
VALUES ('2026-04-15 10:05:00Z', 42, 50);
-- Verify changelog has the pending change within the transaction
SELECT COUNT(*) > 0 as has_pending FROM spiral.changelog WHERE base_view = 'acid_ticks';
ROLLBACK;

-- Verify changelog is clean for this relation outside the transaction
SELECT COUNT(*) as pending_after_rollback FROM spiral.changelog WHERE base_view = 'acid_ticks';

-- Refresh does not alter data based on rolled back transaction
SELECT spiral_refresh('acid_ticks_1h');
SELECT price FROM acid_ticks_1h WHERE symbol_id = 42;
