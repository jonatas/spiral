-- ==============================================================================
-- Spiral Multi-Domain Demo Light — DDL + Sample Functions
-- ==============================================================================
-- Five tables across Finance, IoT, Metrics, Energy, Events.
-- Each table ships two companion functions:
--
--   {table}_sample()              → SETOF {table}  — seed rows; defines cardinality
--   {table}_next_sample(prev, at) → {table}         — random walk from prior state
--
-- Load order:
--   psql -f examples/multi_domain_demo_light.sql           -- DDL + functions
--   psql -f examples/multi_domain_demo_light_emulator.sql  -- heartbeat + tick
--   psql -c "SELECT spiral.bootstrap(60);"                 -- seed history (1 h)
--   psql -c "SELECT spiral.enable_heartbeat();"            -- returns shell command
-- ==============================================================================
LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral;
SET client_min_messages = warning;

-- ── Teardown ──────────────────────────────────────────────────────────────────
-- Snapshot all Spiral-managed tables from the catalog BEFORE draining it.
-- We read spiral.sources AND spiral.metadata to catch every registered table.
CREATE TEMP TABLE _spiral_teardown AS
SELECT DISTINCT unnest(ARRAY[base_view, view_name]) AS tablename
FROM   spiral.sources
UNION
SELECT DISTINCT view_name  FROM spiral.metadata
UNION
SELECT DISTINCT base_view  FROM spiral.metadata;

-- Drain changelog for those tables.
-- DELETE is row-level (MVCC): no conflict with the worker's concurrent SELECT.
DELETE FROM spiral.changelog
WHERE base_view IN (SELECT tablename FROM _spiral_teardown);

SELECT pg_sleep(1.5); -- one full worker tick so in-flight locks release

-- Cancel any still-running worker queries (catches the in-flight tick).
SELECT pg_cancel_backend(pid)
FROM   pg_stat_activity
WHERE  backend_type LIKE 'Spiral Worker%';

-- Drop all collected Spiral tables (base + hierarchy).
-- The DROP TABLE hook (remove_table_from_spiral) now also drops hierarchy tables
-- and cancels worker queries per-table, so this is clean even without retries.
DO $$
DECLARE t text;
BEGIN
    FOR t IN SELECT tablename FROM _spiral_teardown ORDER BY 1 LOOP
        EXECUTE format('DROP TABLE IF EXISTS %I CASCADE', t);
    END LOOP;
END;
$$;

DROP TABLE IF EXISTS _spiral_teardown;

DELETE FROM spiral.metadata WHERE view_name NOT IN (
    SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'
);
SET client_min_messages = notice;

-- ── 1. FINANCE — trades ───────────────────────────────────────────────────────
-- Tenants: symbol  (AAPL · GOOGL · TSLA)
CREATE TABLE trades (
    t       timestamptz NOT NULL,
    symbol  text        NOT NULL,
    price   float8,                -- ohlcv
    volume  float8,                -- sum
    side    smallint
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'symbol');

CREATE OR REPLACE FUNCTION trades_sample()
RETURNS SETOF trades AS $$
    SELECT now(), 'AAPL',  185.0, 1000.0,  1::smallint
    UNION ALL
    SELECT now(), 'GOOGL', 175.0,  800.0, -1::smallint
    UNION ALL
    SELECT now(), 'TSLA',  250.0, 1200.0,  1::smallint;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION trades_next_sample(prev trades, at timestamptz DEFAULT now())
RETURNS trades AS $$
    SELECT
        at,
        prev.symbol,
        GREATEST(0.01, prev.price  + (random() - 0.5) * 2.0),
        GREATEST(1.0,  prev.volume * (0.8 + random() * 0.4)),
        CASE WHEN random() > 0.5 THEN 1::smallint ELSE -1::smallint END;
$$ LANGUAGE sql;

INSERT INTO trades SELECT * FROM trades_sample();

-- ── 2. IoT — iot_readings ─────────────────────────────────────────────────────
-- Tenants: device_id  (1 · 2 · 3)
CREATE TABLE iot_readings (
    t          timestamptz NOT NULL,
    device_id  int         NOT NULL,
    location   text        NOT NULL,
    temp       float8,              -- ohlcv
    humidity   float8,              -- stats
    pressure   float8,
    battery    float8               -- stats
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'device_id');

CREATE OR REPLACE FUNCTION iot_readings_sample()
RETURNS SETOF iot_readings AS $$
    SELECT now(), 1, 'lab-A',   22.0, 50.0, 1013.0, 85.0
    UNION ALL
    SELECT now(), 2, 'lab-B',   21.5, 55.0, 1012.0, 90.0
    UNION ALL
    SELECT now(), 3, 'field-C', 18.0, 70.0, 1010.0, 60.0;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION iot_readings_next_sample(prev iot_readings, at timestamptz DEFAULT now())
RETURNS iot_readings AS $$
    SELECT
        at,
        prev.device_id,
        prev.location,
        prev.temp     + (random() - 0.5) * 0.5,
        LEAST(100, GREATEST(0, prev.humidity + (random() - 0.5) * 2)),
        1013.0        + (random() - 0.5) * 3,
        LEAST(100, GREATEST(0, prev.battery  - random() * 0.02));
$$ LANGUAGE sql;

INSERT INTO iot_readings SELECT * FROM iot_readings_sample();

-- ── 3. METRICS — api_metrics ──────────────────────────────────────────────────
-- Tenants: service  (svc-A · svc-B · svc-C)
CREATE TABLE api_metrics (
    t           timestamptz NOT NULL,
    service     text        NOT NULL,
    status_code int,
    duration_ms float8,             -- stats
    bytes_sent  float8,             -- sum
    error_count float8              -- sum
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'service');

CREATE OR REPLACE FUNCTION api_metrics_sample()
RETURNS SETOF api_metrics AS $$
    SELECT now(), 'svc-A', 200, 25.0,  512.0, 0.0
    UNION ALL
    SELECT now(), 'svc-B', 200, 40.0, 1024.0, 0.0
    UNION ALL
    SELECT now(), 'svc-C', 200, 15.0,  256.0, 0.0;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION api_metrics_next_sample(prev api_metrics, at timestamptz DEFAULT now())
RETURNS api_metrics AS $$
    SELECT
        at,
        prev.service,
        CASE WHEN random() > 0.97 THEN 500
             WHEN random() > 0.93 THEN 429
             ELSE 200 END,
        GREATEST(1.0, prev.duration_ms * (0.5 + random())),
        GREATEST(0.0, prev.bytes_sent  * (0.8 + random() * 0.4)),
        CASE WHEN random() > 0.95 THEN 1.0 ELSE 0.0 END;
$$ LANGUAGE sql;

INSERT INTO api_metrics SELECT * FROM api_metrics_sample();

-- ── 4. ENERGY — energy_grid ───────────────────────────────────────────────────
-- Tenants: zone_id  (1 · 2)
CREATE TABLE energy_grid (
    t           timestamptz NOT NULL,
    zone_id     int         NOT NULL,
    load_mw     float8,             -- stats
    solar_mw    float8,             -- sum
    wind_mw     float8,
    storage_mwh float8,
    price_mwh   float8              -- ohlcv
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'zone_id');

CREATE OR REPLACE FUNCTION energy_grid_sample()
RETURNS SETOF energy_grid AS $$
    SELECT now(), 1, 500.0, 400.0, 200.0, 1000.0, 45.0
    UNION ALL
    SELECT now(), 2, 350.0, 250.0, 150.0,  800.0, 52.0;
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION energy_grid_next_sample(prev energy_grid, at timestamptz DEFAULT now())
RETURNS energy_grid AS $$
    SELECT
        at,
        prev.zone_id,
        GREATEST(0, prev.load_mw     + (random() - 0.5) * 20),
        GREATEST(0, prev.solar_mw    + (random() - 0.5) * 30),
        GREATEST(0, prev.wind_mw     + (random() - 0.5) * 15),
        GREATEST(0, prev.storage_mwh + (random() - 0.5) * 50),
        GREATEST(0, prev.price_mwh   + (random() - 0.5) *  3);
$$ LANGUAGE sql;

INSERT INTO energy_grid SELECT * FROM energy_grid_sample();

-- ── 5. EVENTS — user_events ───────────────────────────────────────────────────
-- Tenants: user_id  (1 · 2 · 3 · 4 · 5)
CREATE TABLE user_events (
    t           timestamptz NOT NULL,
    user_id     int         NOT NULL,
    event_type  text        NOT NULL,
    page        text,
    duration_ms float8,             -- stats
    country     text
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'user_id');

CREATE OR REPLACE FUNCTION user_events_sample()
RETURNS SETOF user_events AS $$
    SELECT now(), 1, 'click',  '/home',       500.0, 'US'
    UNION ALL
    SELECT now(), 2, 'view',   '/dashboard',  800.0, 'BR'
    UNION ALL
    SELECT now(), 3, 'scroll', '/settings',   300.0, 'DE'
    UNION ALL
    SELECT now(), 4, 'click',  '/profile',    400.0, 'JP'
    UNION ALL
    SELECT now(), 5, 'submit', '/home',       250.0, 'FR';
$$ LANGUAGE sql;

CREATE OR REPLACE FUNCTION user_events_next_sample(prev user_events, at timestamptz DEFAULT now())
RETURNS user_events AS $$
    SELECT
        at,
        prev.user_id,
        (ARRAY['click','scroll','view','submit'])[1 + floor(random() * 4)::int],
        (ARRAY['/home','/dashboard','/settings','/profile'])[1 + floor(random() * 4)::int],
        GREATEST(10.0, prev.duration_ms * (0.7 + random() * 0.6)),
        prev.country;
$$ LANGUAGE sql;

INSERT INTO user_events SELECT * FROM user_events_sample();

-- ── Summary ───────────────────────────────────────────────────────────────────
\echo '=== Tables ==='
SELECT m.view_name AS table_name, m.scope_columns AS tenant
FROM spiral.metadata m WHERE m.parent_view = 'BASE';

-- ==============================================================================
-- STORAGE COMPARISON — pg_attribute-derived heap metrics vs Spiral
-- ==============================================================================
-- spiral_get_storage_stats() computes heap_bytes_per_row by walking pg_attribute
-- with PostgreSQL's actual typalign rules — no hardcoded estimates.
-- Run SELECT spiral.bootstrap(60) after loading the emulator for richer data.
-- ==============================================================================

\echo '=== Storage Comparison: heap_bytes_per_row from pg_attribute + pg_type ==='
SELECT
    m.view_name                                                                      AS spiral_table,
    (SELECT count(*) FROM pg_attribute att
     WHERE att.attrelid = c.oid AND att.attnum > 0 AND NOT att.attisdropped)::int   AS col_count,
    (j ->> 'heap_bytes_per_row')::float8                                             AS heap_bpr,
    (j ->> 'heap_rows_per_page')::numeric(8,1)                                       AS heap_rows_per_pg,
    (j ->> 'xor_bytes_per_row')::numeric(6,3)                                        AS xor_bpr,
    (j ->> 'xor_rows_per_page')::float8::int                                         AS xor_rows_per_pg,
    (j ->> 'data_per_page')::int                                                     AS spiral_rows_per_pg
FROM spiral.metadata m
JOIN pg_class c ON c.relname = m.view_name
CROSS JOIN LATERAL (
    SELECT spiral_get_storage_stats(c.oid::int)::jsonb AS j
) s
WHERE m.parent_view = 'BASE'
ORDER BY m.view_name;

\echo '=== Live storage: iot_readings (Spiral actual vs heap projection) ==='
SELECT
    pg_size_pretty((j ->> 'spiral_size_kb')::bigint * 1024)         AS spiral_size,
    pg_size_pretty((j ->> 'projected_heap_size_kb')::bigint * 1024) AS heap_projected,
    (j ->> 'compression_ratio')::numeric(6,2)                        AS compression_ratio,
    (j ->> 'total_rows_capacity')::bigint                            AS rows_capacity,
    (j ->> 'heap_bytes_per_row')::float8                             AS heap_bpr,
    (j ->> 'data_per_page')::int                                     AS spiral_rows_per_page
FROM (SELECT spiral_get_storage_stats('iot_readings'::regclass::oid::int)::jsonb AS j) t;

\echo '=== Column alignment walk for iot_readings ==='
SELECT
    att.attname,
    att.attlen::int                                                                  AS attlen,
    typ.typalign::text                                                               AS typalign,
    CASE typ.typalign WHEN 'd' THEN 8 WHEN 'i' THEN 4 WHEN 's' THEN 2 ELSE 1 END   AS align_bytes,
    pg_catalog.format_type(att.atttypid, att.atttypmod)                             AS type_name
FROM pg_attribute att
JOIN pg_type typ ON att.atttypid = typ.oid
WHERE att.attrelid = 'iot_readings'::regclass AND att.attnum > 0 AND NOT att.attisdropped
ORDER BY att.attnum;
