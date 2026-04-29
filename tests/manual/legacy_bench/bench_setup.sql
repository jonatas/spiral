-- bench_setup.sql

-- 1. Setup Base Tables
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE baseline_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price f64,
    vol int
);

CREATE TABLE spiral_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price f64,
    vol int
);

-- 2. Setup Spiral Hierarchy on spiral_ticks
-- This automatically creates the tracking trigger on spiral_ticks
CREATE MATERIALIZED VIEW stocks_bench_1m AS 
SELECT 
    (spiral(t)/60)*60 as t, 
    symbol_id,
    first(price) as o, max(price) as h, min(price) as l, last(price) as c,
    sum(vol) as volume
FROM spiral_ticks GROUP BY 1, 2
WITH (spiral.frames='5m,1h');
