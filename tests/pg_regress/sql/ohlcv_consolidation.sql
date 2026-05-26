-- Test for Consolidated OHLCV and Heuristic Mapping
SELECT spiral_register_view_rust('ohlcv_source', 'BASE', 0, 'ohlcv_source', ARRAY[]::text[]);

CREATE TABLE ohlcv_source (
    t timestamptz,
    price double precision -- Spiral: ohlcv
);

-- Insert some data
INSERT INTO ohlcv_source (t, price) VALUES 
('2026-05-25 10:00:00+00', 100.0),
('2026-05-25 10:01:00+00', 110.0),
('2026-05-25 10:02:00+00', 105.0),
('2026-05-25 10:03:00+00', 115.0),
('2026-05-25 10:04:00+00', 112.0);

-- Trigger rollup creation
SELECT spiral_register_view_rust('ohlcv_1h', 'ohlcv_source', 3600, 'ohlcv_source', ARRAY[]::text[]);

-- Manually populate the rollup (simulating bgworker)
INSERT INTO ohlcv_1h (t, price_ohlcv)
SELECT 
    date_trunc('hour', t),
    spiral_ohlcv(price, spiral(t))
FROM ohlcv_source
GROUP BY 1;

-- Verify the reconstruction view expansion
SELECT price_ohlcv_o, price_ohlcv_h, price_ohlcv_l, price_ohlcv_c, price_ohlcv_v 
FROM ohlcv_1h_view;

-- Verify planner acceleration and heuristic mapping
-- This should be accelerated and rewritten to use accessor functions on the consolidated state
EXPLAIN (COSTS OFF)
SELECT 
    first(price, spiral(t)),
    max(price),
    min(price),
    last(price, spiral(t)),
    sum(price)
FROM ohlcv_source
WHERE t >= '2026-05-25 10:00:00+00' AND t < '2026-05-25 11:00:00+00';

-- Execute the query
SELECT 
    first(price, spiral(t)),
    max(price),
    min(price),
    last(price, spiral(t)),
    sum(price)
FROM ohlcv_source
WHERE t >= '2026-05-25 10:00:00+00' AND t < '2026-05-25 11:00:00+00';
