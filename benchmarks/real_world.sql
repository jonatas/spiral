-- Spiral Real-World Accuracy & Performance Benchmark
-- Dataset: 100 Million Rows (1 Year of data for 100 Servers)
SET client_min_messages TO NOTICE;
LOAD 'spiral';

SET spiral.kickoff_date = '2025-01-01';

-- 1. Setup
DROP TABLE IF EXISTS metrics_raw CASCADE;
CREATE TABLE metrics_raw (
    t timestamptz NOT NULL,
    host_id text NOT NULL,
    cpu_usage double precision,
    mem_usage double precision
);

-- Register Hierarchy (Multi-tenant by host_id)
SELECT spiral_register_view('metrics_1m'::text, 'BASE'::text, 60::integer, 'metrics_raw'::text, ARRAY['host_id']::text[]);
SELECT spiral_register_view('metrics_1h'::text, 'metrics_1m'::text, 3600::integer, 'metrics_raw'::text, ARRAY['host_id']::text[]);
SELECT spiral_register_view('metrics_1d'::text, 'metrics_1h'::text, 86400::integer, 'metrics_raw'::text, ARRAY['host_id']::text[]);

-- 2. Parallel Realistic Ingestion
\echo '--- [STAGE 1] Ingesting 100M Realistic Rows (1 Year, 100 Hosts) ---'
CREATE OR REPLACE PROCEDURE ingest_real_world()
LANGUAGE plpgsql AS $$
DECLARE
    batch_size INT := 1000000;
    total_rows BIGINT := 20000000;
    current_row BIGINT := 0;
BEGIN
    WHILE current_row < total_rows LOOP
        INSERT INTO metrics_raw (t, host_id, cpu_usage, mem_usage)
        SELECT 
            '2025-01-01 00:00:00Z'::timestamptz + ( (current_row + n) * 1.5 || ' seconds')::interval, -- Spread over a smaller range
            'host_' || ((current_row + n) % 100)::text,
            (sin((current_row + n)::double precision / 1000) * 50) + 50 + (random() * 5),
            random() * 16384
        FROM generate_series(0, batch_size - 1) n;
        
        current_row := current_row + batch_size;
        COMMIT;
        RAISE NOTICE 'Ingestion Progress: % / % rows (%)', current_row, total_rows, (current_row * 100 / total_rows);
    END LOOP;
END;
$$;

\timing on
CALL ingest_real_world();
\timing off

-- 3. Materialize
\echo '--- [STAGE 2] Materializing Hierarchy ---'
\timing on
SELECT spiral_refresh('metrics_1m');
\timing off

-- 4. Accuracy & Performance Comparison
\echo '--- [STAGE 3] ACCURACY & PERFORMANCE CHECK ---'

-- Query: Average CPU for host_42 over a 3-day period
\echo '-> Query: AVG CPU for host_42 (3 Day range)'
\echo '-> Baseline: Raw Scan'
SET spiral.skip_acceleration = true;
\timing on
SELECT avg(cpu_usage) as raw_avg, count(*) as raw_count 
FROM metrics_raw 
WHERE host_id = 'host_42' 
AND t >= '2025-01-05 00:00:00Z' AND t < '2025-01-08 00:00:00Z';
\timing off

\echo '-> Accelerated: Spiral Multi-Tier'
SET spiral.skip_acceleration = false;
\timing on
SELECT avg(cpu_usage) as asp_avg, count(*) as asp_count 
FROM metrics_raw 
WHERE host_id = 'host_42' 
AND t >= '2025-01-05 00:00:00Z' AND t < '2025-01-08 00:00:00Z';
\timing off

-- 5. Join Accuracy Check
\echo '--- [STAGE 4] MULTI-TABLE JOIN ACCURACY ---'
\echo '-> Query: Joining host_1 and host_2 CPU usages over 1 day'
\echo '-> Baseline: Raw Join'
SET spiral.skip_acceleration = true;
\timing on
SELECT sum(m1.cpu_usage + m2.cpu_usage) 
FROM metrics_raw m1
JOIN metrics_raw m2 ON m1.t = m2.t
WHERE m1.host_id = 'host_1' AND m2.host_id = 'host_2'
AND m1.t >= '2025-01-10 00:00:00Z' AND m1.t < '2025-01-11 00:00:00Z';
\timing off

\echo '-> Accelerated: Spiral Dual-Side Propagation'
SET spiral.skip_acceleration = false;
\timing on
SELECT sum(m1.cpu_usage + m2.cpu_usage) 
FROM metrics_raw m1
JOIN metrics_raw m2 ON m1.t = m2.t
WHERE m1.host_id = 'host_1' AND m2.host_id = 'host_2'
AND m1.t >= '2025-01-10 00:00:00Z' AND m1.t < '2025-01-11 00:00:00Z';
\timing off
