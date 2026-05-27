LOAD 'spiral';
CREATE TABLE my_stats (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    latency double precision -- Spiral: stats
) WITH (
    spiral.frames = '1m',
    spiral.tenant = 'symbol_id'
);
INSERT INTO my_stats (t, symbol_id, latency) VALUES ('2026-05-27 10:00:00Z', 1, 100), ('2026-05-27 10:00:10Z', 1, 110);
SELECT spiral_refresh('my_stats');
SELECT row_to_json(d) FROM my_stats_1m d LIMIT 1;
