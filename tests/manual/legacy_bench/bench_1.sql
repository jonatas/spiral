DROP EXTENSION IF EXISTS spiral CASCADE;
DROP TABLE IF EXISTS spiral_ticks CASCADE;
CREATE EXTENSION spiral;
SET spiral.kickoff_date = '2026-04-15';
CREATE UNLOGGED TABLE spiral_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);
INSERT INTO spiral_ticks (t, symbol_id, price, vol) 
SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.001 seconds'), (i % 10), 60000 + sin(i::float/1000)*1000 + random()*100, (random()*100)::int
FROM generate_series(0, 9999999) i;
\echo 'Ingest done'
