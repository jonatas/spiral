-- Benchmark: Adaptive Z-Order Scaling
-- Comparing Auto-Scaling behavior for different ingestion rates.

DROP EXTENSION IF EXISTS aspiral CASCADE;
CREATE EXTENSION aspiral;
SET aspiral.kickoff_date = '2026-04-15';

-- 1. High-Frequency Table (1 event/sec)
CREATE TABLE high_freq (t timestamptz, sensor_id int, val float);
INSERT INTO high_freq SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '1 second'), (i % 10), random() FROM generate_series(0, 9999) i;
ANALYZE high_freq;

-- 2. Low-Frequency Table (1 event/min)
CREATE TABLE low_freq (t timestamptz, sensor_id int, val float);
INSERT INTO low_freq SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '1 minute'), (i % 10), random() FROM generate_series(0, 9999) i;
ANALYZE low_freq;

-- 3. Adaptive Z-Order Test
DO $$
DECLARE
    res_high int;
    res_low int;
    t_val timestamptz := '2026-04-15 00:05:00Z';
BEGIN
    RAISE NOTICE '--- High Frequency Adaptive Z-Order ---';
    -- This should use a small scale (around 10s if we have 10000 points and total time is 10000s)
    -- Our formula: (max-min) / 1000. 
    -- High freq: 10000s / 1000 = 10s scale.
    -- Low freq: 600000s / 1000 = 600s scale.
    
    RAISE NOTICE 'High-Freq Z-Order for %: %', t_val, aspiral_zorder_adaptive(aspiral(t_val), 'high_freq', ARRAY['1']);
    
    RAISE NOTICE '--- Low Frequency Adaptive Z-Order ---';
    RAISE NOTICE 'Low-Freq Z-Order for %: %', t_val, aspiral_zorder_adaptive(aspiral(t_val), 'low_freq', ARRAY['1']);
END $$;
