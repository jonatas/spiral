-- Issue 95 repro
CREATE TABLE acts3 (
    t timestamptz NOT NULL,
    user_id bigint NOT NULL,
    tracked integer DEFAULT 0 -- Spiral: sum, stats as tracked_stats
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'user_id');
INSERT INTO acts3 SELECT '2024-01-01'::timestamptz + interval '10 min' * g, (g % 5) + 1, 60
FROM generate_series(0, 50000) g;
SELECT spiral_refresh('acts3');

-- post-refresh write -> dirty range -> mixed raw+tier segments
INSERT INTO acts3 VALUES ('2024-05-01 00:05:00+00', 1, 60);

SELECT date_trunc('day', t), user_id, sum(tracked)
FROM acts3 WHERE t >= '2024-02-01' AND t < '2024-06-01' GROUP BY 1,2;
