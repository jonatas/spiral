DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

DROP TABLE IF EXISTS bg_guc_ticks CASCADE;
DROP TABLE IF EXISTS bg_guc_ticks_1m CASCADE;

CREATE TABLE bg_guc_ticks (
    t timestamptz NOT NULL,
    symbol_id int NOT NULL,
    price numeric -- Spiral: sum
) WITH (spiral.frames = '1m', spiral.tenant = 'symbol_id');

-- Disable the background worker using the GUC globally
ALTER SYSTEM SET spiral.worker_enabled = false;
SELECT pg_reload_conf();

-- Wait for the worker to receive the SIGHUP and reload config
SELECT pg_sleep(2);

INSERT INTO bg_guc_ticks (t, symbol_id, price) VALUES ('2026-04-15 10:05:00Z', 42, 100);

-- Wait enough time for the worker to poll
SELECT pg_sleep(3.5);

-- Check that it is NOT processed because the worker is paused
SELECT count(*) = 0 as should_be_paused FROM bg_guc_ticks_1m;

-- Enable the background worker and set debug logging globally
ALTER SYSTEM SET spiral.worker_enabled = true;
ALTER SYSTEM SET spiral.worker_debug = true;
SELECT pg_reload_conf();

-- Wait enough time for the worker to process it
SELECT pg_sleep(3.5);

-- Check that it IS processed now
SELECT count(*) = 1 as should_be_processed FROM bg_guc_ticks_1m;
