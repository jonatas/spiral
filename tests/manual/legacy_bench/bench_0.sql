DROP TABLE IF EXISTS vanilla_ticks CASCADE;
CREATE UNLOGGED TABLE vanilla_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);
INSERT INTO vanilla_ticks (t, symbol_id, price, vol) 
SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
FROM generate_series(0, 9999999) i;
\echo 'Vanilla Ingest done'
