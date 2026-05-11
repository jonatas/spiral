DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral;

ALTER SYSTEM SET spiral.worker_enabled = true;
ALTER SYSTEM SET spiral.max_workers = 4;
SELECT pg_reload_conf();

-- Wait for workers to reload config
SELECT pg_sleep(1);

DROP TABLE IF EXISTS bg_par_1 CASCADE;
DROP TABLE IF EXISTS bg_par_2 CASCADE;
DROP TABLE IF EXISTS bg_par_3 CASCADE;
DROP TABLE IF EXISTS bg_par_1_1m CASCADE;
DROP TABLE IF EXISTS bg_par_2_1m CASCADE;
DROP TABLE IF EXISTS bg_par_3_1m CASCADE;

CREATE TABLE bg_par_1 (t timestamptz NOT NULL, id int NOT NULL, val numeric) WITH (spiral.frames='1m', spiral.tenant='id');
CREATE TABLE bg_par_2 (t timestamptz NOT NULL, id int NOT NULL, val numeric) WITH (spiral.frames='1m', spiral.tenant='id');
CREATE TABLE bg_par_3 (t timestamptz NOT NULL, id int NOT NULL, val numeric) WITH (spiral.frames='1m', spiral.tenant='id');

-- Insert into all tables
INSERT INTO bg_par_1 VALUES ('2026-04-15 10:05:00Z', 1, 100);
INSERT INTO bg_par_2 VALUES ('2026-04-15 10:05:00Z', 1, 100);
INSERT INTO bg_par_3 VALUES ('2026-04-15 10:05:00Z', 1, 100);

-- They should be processed in parallel. Sleep to wait.
SELECT pg_sleep(3.5);

-- Check results
SELECT COUNT(*) = 1 as p1 FROM bg_par_1_1m;
SELECT COUNT(*) = 1 as p2 FROM bg_par_2_1m;
SELECT COUNT(*) = 1 as p3 FROM bg_par_3_1m;

-- Check how many workers are active (should be up to max_workers)
SELECT count(*) > 0 as has_workers FROM pg_stat_activity WHERE backend_type LIKE 'Spiral Worker%';
