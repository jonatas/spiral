LOAD 'spiral';
SET spiral.kickoff_date = '2026-01-01 00:00:00+00';

CREATE TABLE cross_moment_test (
    t timestamptz NOT NULL,
    price double precision, -- Spiral: ohlcv
    vol double precision    -- Spiral: sum, product(price, vol)
);

-- We want to support:
-- SUM(price * vol) AS weighted_sum

-- Attempt to accelerate
SELECT accelerate('cross_moment_test', '1h');

INSERT INTO cross_moment_test (t, price, vol)
VALUES 
    ('2026-01-01 00:30:00+00', 10.0, 5.0),
    ('2026-01-01 00:45:00+00', 12.0, 4.0);

SELECT refresh('cross_moment_test');

-- Query with product sum and time constraint to trigger acceleration
SELECT spiral_explain('SELECT SUM(price * vol) FROM cross_moment_test WHERE t >= ''2026-01-01 00:00:00+00'' AND t < ''2026-01-02 00:00:00+00''');

SELECT SUM(price * vol) FROM cross_moment_test WHERE t >= '2026-01-01 00:00:00+00' AND t < '2026-01-02 00:00:00+00';

-- Query with multiple aggregates including product
SELECT SUM(vol), SUM(price * vol), MAX(price) FROM cross_moment_test WHERE t >= '2026-01-01 00:00:00+00' AND t < '2026-01-02 00:00:00+00';

DROP TABLE cross_moment_test CASCADE;
