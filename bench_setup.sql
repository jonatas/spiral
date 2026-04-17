-- bench_setup.sql

-- 1. Setup Base Tables
SET aspiral.kickoff_date = '2026-04-15';

CREATE TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price f64,
    vol int
);

CREATE TABLE aspiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price f64,
    vol int
);

-- 2. Setup Aspiral Hierarchy on aspiral_ticks
-- This automatically creates the tracking trigger on aspiral_ticks
CREATE MATERIALIZED VIEW stocks_bench_1m AS 
SELECT 
    (aspiral(t)/60)*60 as t, 
    symbol_id,
    first(price) as o, max(price) as h, min(price) as l, last(price) as c,
    sum(vol) as volume
FROM aspiral_ticks GROUP BY 1, 2
WITH (aspiral.frames='5m,1h');
