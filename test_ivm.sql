-- Aspiral IVM (MERGE Strategy) Test
DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;

-- Thorough cleanup
DROP TABLE IF EXISTS ivm_ticks CASCADE;
DROP TABLE IF EXISTS ivm_ticks_ohlcv_1h CASCADE;

CREATE TABLE ivm_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric -- Aspiral: ohlc, sum
) WITH (aspiral.frames = '1h', aspiral.tenant = 'symbol_id');

-- 1. Initial Ingestion (Last hour)
INSERT INTO ivm_ticks (t, symbol_id, price)
SELECT 
    now() - interval '2 hours' + (i * interval '1 second'),
    1,
    100 + random() * 10
FROM generate_series(0, 59) i;

-- Trigger initial refresh
SELECT '--- Initial Refresh ---' as msg;
SELECT aspiral_refresh('ivm_ticks_ohlcv_1h');

-- 2. SURGICAL UPDATE (Incremental MERGE)
INSERT INTO ivm_ticks (t, symbol_id, price)
VALUES (now() - interval '2 hours' + interval '30 seconds', 1, 88888.0);

-- Trigger Incremental
SELECT '--- Incremental MERGE Refresh ---' as msg;
SELECT aspiral_refresh('ivm_ticks_ohlcv_1h');

-- 3. VERIFY RESULTS
SELECT round(price_h::numeric, 2) as high, round(price_sum::numeric, 2) as total FROM ivm_ticks_ohlcv_1h;
