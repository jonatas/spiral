DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS bg_auto_ticks CASCADE;
DROP TABLE IF EXISTS bg_auto_ticks_1m CASCADE;

CREATE TABLE bg_auto_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric -- Spiral: sum
) WITH (spiral.frames = '1m', spiral.tenant = 'symbol_id');

-- Insert initial data
INSERT INTO bg_auto_ticks (t, symbol_id, price)
VALUES ('2026-04-15 10:05:00Z', 42, 100);

-- Wait for the background worker to wake up and process (it runs every 1 second)
SELECT pg_sleep(2.5);

-- Verify the background worker processed it
SELECT price FROM bg_auto_ticks_1m WHERE symbol_id = 42;
