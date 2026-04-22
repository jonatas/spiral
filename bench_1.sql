DROP EXTENSION IF EXISTS aspiral CASCADE;
DROP TABLE IF EXISTS aspiral_ticks CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';
CREATE UNLOGGED TABLE aspiral_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);
INSERT INTO aspiral_ticks (t, symbol_id, price, vol) 
SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
FROM generate_series(0, 9999999) i;
\echo 'Ingest done'
