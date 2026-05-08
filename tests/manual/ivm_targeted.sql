-- Spiral IVM (Targeted MERGE) Debug Test
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2026-04-01';

DROP TABLE IF EXISTS ivm_ticks CASCADE;
DROP TABLE IF EXISTS ivm_ticks_1h CASCADE;

CREATE TABLE ivm_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric -- Spiral: ohlc, sum
) WITH (spiral.frames = '1h', spiral.tenant = 'symbol_id');

-- 1. Initial Ingestion
INSERT INTO ivm_ticks (t, symbol_id, price)
VALUES ('2026-04-05 08:00:00Z', 1, 100.0);
INSERT INTO ivm_ticks (t, symbol_id, price)
VALUES ('2026-04-05 08:00:00Z', 2, 200.0);

SELECT t, spiral(t) as a_t FROM ivm_ticks;

-- Initial Sync
SELECT spiral_refresh('ivm_ticks');

-- 2. TARGETED UPDATE
INSERT INTO ivm_ticks (t, symbol_id, price)
VALUES ('2026-04-05 08:05:30Z', 1, 88888.0);
INSERT INTO ivm_ticks (t, symbol_id, price)
VALUES ('2026-04-05 08:05:30Z', 2, 77777.0);

-- Refresh ONLY Symbol 1
SELECT '--- Targeted Refresh for Symbol 1 ---' as msg;
SELECT spiral_refresh('ivm_ticks', 'symbol_id = 1');

-- 3. VERIFY RESULTS
-- Display raw counts first
SELECT count(*) FROM ivm_ticks_1h;

SELECT symbol_id, round(price_ohlcv_h::numeric, 2) as high FROM ivm_ticks_1h ORDER BY symbol_id;
