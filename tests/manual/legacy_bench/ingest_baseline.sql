-- ingest_baseline.sql
INSERT INTO baseline_ticks (t, symbol_id, price, vol) 
VALUES (now(), 1 + floor(random() * 10)::int, random() * 100, (random() * 1000)::int);
