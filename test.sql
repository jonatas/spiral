LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral;
CREATE TABLE my_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric, -- Spiral: ohlcv
    volume int -- Spiral: sum
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'symbol_id'
);
INSERT INTO my_ticks (t, symbol_id, price, volume) VALUES ('2026-05-27 10:00:00Z', 1, 100, 10), ('2026-05-27 10:00:10Z', 1, 105, 20);
SELECT spiral_refresh('my_ticks');
SELECT row_to_json(d) FROM my_ticks_1m d LIMIT 1;
