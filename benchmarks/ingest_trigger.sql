-- Ingestion benchmark to test trigger performance
DROP EXTENSION IF EXISTS spiral CASCADE;
CREATE EXTENSION spiral CASCADE;
SET spiral.kickoff_date = '2026-04-15';

DROP TABLE IF EXISTS trigger_ticks CASCADE;
DROP TABLE IF EXISTS trigger_1m CASCADE;

CREATE TABLE trigger_ticks (t timestamptz NOT NULL, symbol_id int NOT NULL, price double precision, vol int);
CREATE UNLOGGED TABLE trigger_1m AS 
SELECT to_timestamptz((spiral(t)/60)*60) as t, symbol_id, sum(vol) as volume
FROM trigger_ticks GROUP BY 1, 2;

-- Register view, which creates the IVM triggers on trigger_ticks (and tracks changelog)
SELECT spiral_register_view('trigger_1m', 'BASE', 60, 'trigger_ticks', ARRAY['symbol_id']);

-- Simulate what vortex-server used to do (the lock contention source)
CREATE OR REPLACE FUNCTION spiral.spiral_changelog_notify()
 RETURNS trigger LANGUAGE plpgsql AS $$
 BEGIN
   PERFORM pg_notify('spiral_changelog', json_build_object(
     'event_id',    NEW.event_id,
     'base_view',   NEW.base_view,
     'scope_values', NEW.scope_values,
     't_start',     NEW.t_start,
     't_end',       NEW.t_end
   )::text);
   RETURN NEW;
 END;
$$;
CREATE TRIGGER changelog_notify_trigger
 AFTER INSERT ON spiral.changelog
 FOR EACH ROW EXECUTE FUNCTION spiral.spiral_changelog_notify();

DO $$
DECLARE
    rows int := 1000000;
    start_time timestamptz;
    dur interval;
BEGIN
    RAISE NOTICE '--- Starting 1M Row Ingestion (With Triggers) ---';
    start_time := clock_timestamp();
    INSERT INTO trigger_ticks (t, symbol_id, price, vol) 
    SELECT '2026-04-15 00:00:00Z'::timestamptz + (i * interval '0.01 seconds'), (i % 10), 60000, 10
    FROM generate_series(0, rows-1) i;
    dur := clock_timestamp() - start_time;
    RAISE NOTICE 'Ingest with IVM & Changelog: % (% rows/s)', dur, round(rows / extract(epoch from dur));
END $$;
