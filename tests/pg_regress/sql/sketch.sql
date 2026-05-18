LOAD 'spiral';
SET spiral.kickoff_date = '2026-04-15';

-- Basic quantile: exact for small distinct-value dataset
SELECT
    spiral_quantile(spiral_sketch(v), 0.0) AS p0,
    spiral_quantile(spiral_sketch(v), 0.5) AS p50,
    spiral_quantile(spiral_sketch(v), 1.0) AS p100
FROM (VALUES (1.0::float8), (2.0), (3.0), (4.0), (5.0)) AS t(v);

-- Repeated values collapse into a single centroid
SELECT
    spiral_quantile(spiral_sketch(v), 0.5) AS p50,
    spiral_quantile(spiral_sketch(v), 0.9) AS p90
FROM (VALUES (5.0::float8), (5.0), (5.0), (10.0)) AS t(v);

-- min/max/count/sum are always exact
SELECT
    count(*) AS n,
    min(v) AS expected_min,
    max(v) AS expected_max,
    sum(v) AS expected_sum
FROM generate_series(1, 10) AS t(v);

-- Merge two partial sketches; count and sum must match full aggregation
WITH full_sketch AS (
    SELECT spiral_sketch(v::float8) AS sk
    FROM generate_series(1, 6) AS t(v)
),
merged_sketch AS (
    SELECT spiral_sketch_merge(sk) AS sk
    FROM (
        SELECT spiral_sketch(v::float8) AS sk FROM generate_series(1, 3) AS t(v)
        UNION ALL
        SELECT spiral_sketch(v::float8) AS sk FROM generate_series(4, 6) AS t(v)
    ) AS parts
)
SELECT
    round((full_sketch.sk->'count')::numeric, 0)   AS full_count,
    round((merged_sketch.sk->'count')::numeric, 0) AS merged_count,
    round((full_sketch.sk->'sum')::numeric, 2)     AS full_sum,
    round((merged_sketch.sk->'sum')::numeric, 2)   AS merged_sum,
    spiral_quantile(full_sketch.sk, 0.5)           AS full_p50,
    spiral_quantile(merged_sketch.sk, 0.5)         AS merged_p50
FROM full_sketch, merged_sketch;

-- Spiral table with sketch column
CREATE TABLE test_sketch (
    t timestamptz NOT NULL,
    val double precision -- Spiral: sketch
) WITH (spiral.frames = '1m');

INSERT INTO test_sketch (t, val) VALUES
('2026-04-15 00:00:00Z', 10.0),
('2026-04-15 00:00:01Z', 20.0),
('2026-04-15 00:00:02Z', 30.0),
('2026-04-15 00:00:03Z', 40.0),
('2026-04-15 00:00:04Z', 50.0);

SELECT spiral_refresh('test_sketch');

SELECT
    spiral_quantile(val_sketch, 0.0)  AS p0,
    spiral_quantile(val_sketch, 0.5)  AS p50,
    spiral_quantile(val_sketch, 1.0)  AS p100
FROM test_sketch_1m;
