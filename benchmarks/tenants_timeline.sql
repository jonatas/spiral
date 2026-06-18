-- Benchmark for dynamic tenant timeline transitions
-- We will ingest 10M rows across 3 months, simulating exponential tenant growth.
-- Month 1: 10 tenants
-- Month 2: 100 tenants
-- Month 3: 1000 tenants

CREATE EXTENSION IF NOT EXISTS spiral;

-- 1. Create the base table using TAM directly
DROP TABLE IF EXISTS timeline_bench CASCADE;
CREATE TABLE timeline_bench (t timestamptz NOT NULL, tenant_id int NOT NULL, val double precision) USING spiral;

-- 2. Configure Spiral parameters
SET spiral.kickoff_date = '2024-01-01 00:00:00Z';
SET spiral.warn_on_tam_writes = false;

-- Insert initial epoch before data
INSERT INTO spiral.tenants_timeline (table_name, start_t, end_t, tenant_scale, base_offset)
VALUES ('timeline_bench', 0, NULL, 16, 0); 

-- 3. Ingest Data (Month 1: 10 tenants, ~3.3M rows)
DO $$
BEGIN
  RAISE NOTICE 'Ingesting Month 1...';
  INSERT INTO timeline_bench (t, tenant_id, val)
  SELECT 
    '2024-01-01 00:00:00Z'::timestamptz + (s * interval '8 seconds'),
    (s % 10),
    random() * 100
  FROM generate_series(0, 1000) s;
END;
$$;

-- 4. Transition Epoch (Month 2: 100 tenants, ~3.3M rows)
DO $$
DECLARE
  v_start1 BIGINT := 0;
  v_start2 BIGINT := EXTRACT(EPOCH FROM '2024-02-01 00:00:00Z'::timestamptz) - EXTRACT(EPOCH FROM '2024-01-01 00:00:00Z'::timestamptz);
  v_base_offset BIGINT;
BEGIN
  RAISE NOTICE 'Creating Epoch 2 and Ingesting Month 2...';
  -- Calculate offset: 31 days = 2678400s
  -- required slots: 2678400 * 16 = 42854400
  -- TAM_DATA_PER_PAGE = 509 -> pad to 509: 42854582
  v_base_offset := ((2678400 * 16) / 509 + 1) * 509;

  INSERT INTO spiral.tenants_timeline (table_name, start_t, end_t, tenant_scale, base_offset)
  VALUES ('timeline_bench', v_start2, NULL, 128, v_base_offset); 
  
  -- Close previous epoch
  UPDATE spiral.tenants_timeline 
  SET end_t = v_start2
  WHERE table_name = 'timeline_bench' AND end_t IS NULL AND start_t < v_start2;
  
  -- Insert Month 2
  INSERT INTO timeline_bench (t, tenant_id, val)
  SELECT 
    '2024-02-01 00:00:00Z'::timestamptz + (s * interval '8 seconds'),
    (s % 100),
    random() * 100
  FROM generate_series(0, 1000) s;
END;
$$;

-- 5. Transition Epoch (Month 3: 1000 tenants, ~3.3M rows)
DO $$
DECLARE
  v_start2 BIGINT := EXTRACT(EPOCH FROM '2024-02-01 00:00:00Z'::timestamptz) - EXTRACT(EPOCH FROM '2024-01-01 00:00:00Z'::timestamptz);
  v_start3 BIGINT := EXTRACT(EPOCH FROM '2024-03-01 00:00:00Z'::timestamptz) - EXTRACT(EPOCH FROM '2024-01-01 00:00:00Z'::timestamptz);
  v_base_offset BIGINT;
BEGIN
  RAISE NOTICE 'Creating Epoch 3 and Ingesting Month 3...';
  -- Calculate offset: Month 2 has 29 days (2024 is leap year, so 29 days) = 2505600s
  -- required slots = 2505600 * 128 = 320716800. Previous was 42854582
  -- total = 363571382. Pad to 509: 363571866
  v_base_offset := ((363571382) / 509 + 1) * 509;

  INSERT INTO spiral.tenants_timeline (table_name, start_t, end_t, tenant_scale, base_offset)
  VALUES ('timeline_bench', v_start3, NULL, 1024, v_base_offset); 
  
  -- Close previous epoch
  UPDATE spiral.tenants_timeline 
  SET end_t = v_start3
  WHERE table_name = 'timeline_bench' AND end_t IS NULL AND start_t < v_start3;

  -- Insert Month 3
  INSERT INTO timeline_bench (t, tenant_id, val)
  SELECT 
    '2024-03-01 00:00:00Z'::timestamptz + (s * interval '8 seconds'),
    (s % 1000),
    random() * 100
  FROM generate_series(0, 1000) s;
END;
$$;

-- 7. Query tests
DO $$ BEGIN RAISE NOTICE 'Querying Across Epochs...'; END $$;
EXPLAIN ANALYZE
SELECT date_trunc('day', t), count(*) 
FROM timeline_bench 
WHERE tenant_id = 5 
GROUP BY 1 
ORDER BY 1;