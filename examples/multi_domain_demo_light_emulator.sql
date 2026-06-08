-- ==============================================================================
-- Spiral Multi-Domain Demo Light — Heartbeat Engine
-- ==============================================================================
-- Provides tick/bootstrap/enable_heartbeat on top of the sample pattern defined
-- in multi_domain_demo_light.sql.
--
-- Usage:
--   \i examples/multi_domain_demo_light.sql           -- DDL + sample/next_sample
--   \i examples/multi_domain_demo_light_emulator.sql  -- heartbeat engine
--   SELECT spiral.bootstrap(60);                      -- 60 rows history per tenant
--   SELECT spiral.enable_heartbeat();                 -- returns shell command
--
-- Pattern:
--   {table}_sample()                → seed rows, defines tenants (cardinality)
--   {table}_next_sample(prev, at)   → next state via random walk
--   {table}_tick(n)                 → advance all tenants n steps at now()
--   {table}_bootstrap(n, lookback)  → seed + build n rows of history
--
--   spiral.tick(n)                  → call all registered table ticks
--   spiral.bootstrap(n, lookback)   → call all registered table bootstraps
--   spiral.enable_heartbeat(s, n)   → configure + return shell command
--   spiral.disable_heartbeat()      → pause
--   spiral.heartbeat_status()       → show config + tick count
--
-- Increase data resolution:
--   SELECT spiral.tick(10);  -- 10 rows per tenant per call
--   SELECT spiral.enable_heartbeat(1, 5);  -- 5 samples/s at 1-second interval
--
-- Background scheduling (future: Rust BG worker reads spiral.heartbeat_config):
--   while true; do psql -c "SELECT spiral.tick();"; sleep 1; done
-- ==============================================================================
LOAD 'spiral';

-- ── Heartbeat infrastructure ──────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS spiral.heartbeat_config (
    id               int     PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    enabled          bool    NOT NULL DEFAULT false,
    interval_s       int     NOT NULL DEFAULT 1,
    samples_per_tick int     NOT NULL DEFAULT 1,
    tick_count       bigint  NOT NULL DEFAULT 0,
    started_at       timestamptz,
    stopped_at       timestamptz
);
INSERT INTO spiral.heartbeat_config DEFAULT VALUES ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS spiral.heartbeat_registry (
    ord          serial PRIMARY KEY,
    table_name   text   UNIQUE NOT NULL,
    sample_fn    text   NOT NULL,
    tick_fn      text   NOT NULL,
    bootstrap_fn text   NOT NULL
);
-- Idempotent reload: drop entries for this demo's tables then re-insert.
DELETE FROM spiral.heartbeat_registry
WHERE table_name IN ('trades','iot_readings','api_metrics','energy_grid','user_events');
-- ── 1. TRADES ─────────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_heartbeat_trades ON trades (symbol, t DESC);

CREATE OR REPLACE FUNCTION trades_tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    prev trades;
    i    int;
BEGIN
    FOR prev IN
        SELECT DISTINCT ON (symbol) * FROM trades ORDER BY symbol, t DESC
    LOOP
        FOR i IN 1..n LOOP
            prev := trades_next_sample(prev);
            INSERT INTO trades VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION trades_bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    seed trades; prev trades; step interval; i int; at timestamptz;
BEGIN
    step := lookback / GREATEST(n, 1);
    FOR seed IN SELECT * FROM trades_sample() LOOP
        prev   := seed;
        prev.t := now() - lookback;
        INSERT INTO trades VALUES (prev.*);
        FOR i IN 1..n LOOP
            at   := prev.t + step;
            prev := trades_next_sample(prev, at);
            INSERT INTO trades VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── 2. IoT READINGS ───────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_heartbeat_iot ON iot_readings (device_id, t DESC);
CREATE OR REPLACE FUNCTION iot_readings_tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    prev iot_readings;
    i    int;
BEGIN
    FOR prev IN
        SELECT DISTINCT ON (device_id) * FROM iot_readings ORDER BY device_id, t DESC
    LOOP
        FOR i IN 1..n LOOP
            prev := iot_readings_next_sample(prev);
            INSERT INTO iot_readings VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION iot_readings_bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    seed iot_readings; prev iot_readings; step interval; i int; at timestamptz;
BEGIN
    step := lookback / GREATEST(n, 1);
    FOR seed IN SELECT * FROM iot_readings_sample() LOOP
        prev   := seed;
        prev.t := now() - lookback;
        INSERT INTO iot_readings VALUES (prev.*);
        FOR i IN 1..n LOOP
            at   := prev.t + step;
            prev := iot_readings_next_sample(prev, at);
            INSERT INTO iot_readings VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── 3. API METRICS ────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_heartbeat_metrics ON api_metrics (service, t DESC);
CREATE OR REPLACE FUNCTION api_metrics_tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    prev api_metrics;
    i    int;
BEGIN
    FOR prev IN
        SELECT DISTINCT ON (service) * FROM api_metrics ORDER BY service, t DESC
    LOOP
        FOR i IN 1..n LOOP
            prev := api_metrics_next_sample(prev);
            INSERT INTO api_metrics VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION api_metrics_bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    seed api_metrics; prev api_metrics; step interval; i int; at timestamptz;
BEGIN
    step := lookback / GREATEST(n, 1);
    FOR seed IN SELECT * FROM api_metrics_sample() LOOP
        prev   := seed;
        prev.t := now() - lookback;
        INSERT INTO api_metrics VALUES (prev.*);
        FOR i IN 1..n LOOP
            at   := prev.t + step;
            prev := api_metrics_next_sample(prev, at);
            INSERT INTO api_metrics VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── 4. ENERGY GRID ────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_heartbeat_energy ON energy_grid (zone_id, t DESC);
CREATE OR REPLACE FUNCTION energy_grid_tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    prev energy_grid;
    i    int;
BEGIN
    FOR prev IN
        SELECT DISTINCT ON (zone_id) * FROM energy_grid ORDER BY zone_id, t DESC
    LOOP
        FOR i IN 1..n LOOP
            prev := energy_grid_next_sample(prev);
            INSERT INTO energy_grid VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION energy_grid_bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    seed energy_grid; prev energy_grid; step interval; i int; at timestamptz;
BEGIN
    step := lookback / GREATEST(n, 1);
    FOR seed IN SELECT * FROM energy_grid_sample() LOOP
        prev   := seed;
        prev.t := now() - lookback;
        INSERT INTO energy_grid VALUES (prev.*);
        FOR i IN 1..n LOOP
            at   := prev.t + step;
            prev := energy_grid_next_sample(prev, at);
            INSERT INTO energy_grid VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── 5. USER EVENTS ────────────────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS idx_heartbeat_events ON user_events (user_id, t DESC);
CREATE OR REPLACE FUNCTION user_events_tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    prev user_events;
    i    int;
BEGIN
    FOR prev IN
        SELECT DISTINCT ON (user_id) * FROM user_events ORDER BY user_id, t DESC
    LOOP
        FOR i IN 1..n LOOP
            prev := user_events_next_sample(prev);
            INSERT INTO user_events VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION user_events_bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    seed user_events; prev user_events; step interval; i int; at timestamptz;
BEGIN
    step := lookback / GREATEST(n, 1);
    FOR seed IN SELECT * FROM user_events_sample() LOOP
        prev   := seed;
        prev.t := now() - lookback;
        INSERT INTO user_events VALUES (prev.*);
        FOR i IN 1..n LOOP
            at   := prev.t + step;
            prev := user_events_next_sample(prev, at);
            INSERT INTO user_events VALUES (prev.*);
        END LOOP;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── Register tables ───────────────────────────────────────────────────────────
INSERT INTO spiral.heartbeat_registry (table_name, sample_fn, tick_fn, bootstrap_fn) VALUES
    ('trades',       'trades_sample',       'trades_tick',       'trades_bootstrap'),
    ('iot_readings', 'iot_readings_sample', 'iot_readings_tick', 'iot_readings_bootstrap'),
    ('api_metrics',  'api_metrics_sample',  'api_metrics_tick',  'api_metrics_bootstrap'),
    ('energy_grid',  'energy_grid_sample',  'energy_grid_tick',  'energy_grid_bootstrap'),
    ('user_events',  'user_events_sample',  'user_events_tick',  'user_events_bootstrap');

-- ── spiral.tick(n) ────────────────────────────────────────────────────────────
-- Advance every registered table by n samples. Chains each tenant's last row
-- through next_sample so the random walk is continuous across calls.
CREATE OR REPLACE FUNCTION spiral.tick(n int DEFAULT 1) RETURNS void AS $$
DECLARE
    r spiral.heartbeat_registry;
BEGIN
    FOR r IN SELECT * FROM spiral.heartbeat_registry ORDER BY ord LOOP
        EXECUTE format('SELECT %I($1)', r.tick_fn) USING n;
    END LOOP;
    UPDATE spiral.heartbeat_config SET tick_count = tick_count + 1 WHERE id = 1;
END;
$$ LANGUAGE plpgsql;

-- ── spiral.bootstrap(n, lookback) ────────────────────────────────────────────
-- Seeds each table with n rows of chained history spread over lookback.
-- Cardinality comes from each table's _sample() function.
-- Call once after loading the emulator; spiral.tick() keeps data live after that.
CREATE OR REPLACE FUNCTION spiral.bootstrap(
    n        int      DEFAULT 60,
    lookback interval DEFAULT '1 hour'
) RETURNS void AS $$
DECLARE
    r spiral.heartbeat_registry;
BEGIN
    FOR r IN SELECT * FROM spiral.heartbeat_registry ORDER BY ord LOOP
        EXECUTE format('SELECT %I($1, $2)', r.bootstrap_fn) USING n, lookback;
        RAISE NOTICE 'bootstrapped %', r.table_name;
    END LOOP;
END;
$$ LANGUAGE plpgsql;

-- ── spiral.enable_heartbeat(interval_s, samples_per_tick) ────────────────────
-- Stores config; returns shell command to drive ticks until a Rust BG worker
-- reads spiral.heartbeat_config and runs spiral.tick() automatically.
CREATE OR REPLACE FUNCTION spiral.enable_heartbeat(
    p_interval_s       int DEFAULT 1,
    p_samples_per_tick int DEFAULT 1
) RETURNS text AS $$
DECLARE
    shell text;
BEGIN
    UPDATE spiral.heartbeat_config
    SET enabled          = true,
        interval_s       = p_interval_s,
        samples_per_tick = p_samples_per_tick,
        started_at       = now(),
        stopped_at       = NULL
    WHERE id = 1;

    shell := format(
        'while true; do psql -c "SELECT spiral.tick(%s);"; sleep %s; done',
        p_samples_per_tick, p_interval_s
    );
    RAISE NOTICE 'Heartbeat enabled (interval=%ss, samples_per_tick=%s). Shell: %',
        p_interval_s, p_samples_per_tick, shell;
    RETURN shell;
END;
$$ LANGUAGE plpgsql;

-- ── spiral.disable_heartbeat() ───────────────────────────────────────────────
CREATE OR REPLACE FUNCTION spiral.disable_heartbeat() RETURNS void AS $$
BEGIN
    UPDATE spiral.heartbeat_config SET enabled = false, stopped_at = now() WHERE id = 1;
    RAISE NOTICE 'Heartbeat disabled.';
END;
$$ LANGUAGE plpgsql;

-- ── spiral.heartbeat_status() ────────────────────────────────────────────────
CREATE OR REPLACE FUNCTION spiral.heartbeat_status()
RETURNS TABLE (key text, value text) AS $$
    SELECT 'enabled'::text,         enabled::text          FROM spiral.heartbeat_config WHERE id = 1
    UNION ALL
    SELECT 'interval_s',            interval_s::text       FROM spiral.heartbeat_config WHERE id = 1
    UNION ALL
    SELECT 'samples_per_tick',      samples_per_tick::text FROM spiral.heartbeat_config WHERE id = 1
    UNION ALL
    SELECT 'tick_count',            tick_count::text       FROM spiral.heartbeat_config WHERE id = 1
    UNION ALL
    SELECT 'started_at',            started_at::text       FROM spiral.heartbeat_config WHERE id = 1
    UNION ALL
    SELECT 'tables_registered',     count(*)::text         FROM spiral.heartbeat_registry;
$$ LANGUAGE sql;

\echo '=== Heartbeat Status ==='
SELECT * FROM spiral.heartbeat_status();

\echo ''
\echo 'Ready. Run:'
\echo '  SELECT spiral.bootstrap(60);        -- seed 60 rows of history per tenant'
\echo '  SELECT spiral.enable_heartbeat();   -- get shell command for live ticks'
\echo '  SELECT spiral.tick(5);              -- manual burst: 5 samples per tenant'
\echo '  SELECT spiral.heartbeat_status();   -- check config + tick count'
