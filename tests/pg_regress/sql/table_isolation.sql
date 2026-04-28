-- Test for table isolation
SET spiral.kickoff_date = '2026-04-15';

CREATE TABLE table1 (
    t timestamptz NOT NULL,
    val double precision -- Spiral: sum
) WITH (spiral.frames = '1m');

CREATE TABLE table2 (
    t timestamptz NOT NULL,
    val double precision -- Spiral: sum
) WITH (spiral.frames = '1m');

INSERT INTO table1 VALUES ('2026-04-15 10:00:05Z', 10.0);
INSERT INTO table2 VALUES ('2026-04-15 10:00:05Z', 20.0);

SELECT spiral_refresh('table1_sum_1m');
SELECT spiral_refresh('table2_sum_1m');

-- Dirty table1
UPDATE table1 SET val = 15.0;

-- Changelog should only show table1
SELECT base_view, count(*) FROM spiral.changelog GROUP BY 1;

-- table2_sum_1m should still be clean and accelerated
SELECT spiral_explain('SELECT sum(val) FROM table2 WHERE t >= ''2026-04-15 10:00:00Z'' AND t < ''2026-04-15 10:01:00Z''');

-- table1_sum_1m should show RAW fallback
SELECT spiral_explain('SELECT sum(val) FROM table1 WHERE t >= ''2026-04-15 10:00:00Z'' AND t < ''2026-04-15 10:01:00Z''');
