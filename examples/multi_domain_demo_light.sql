-- ==============================================================================
-- Spiral Multi-Domain Demo (LIGHT): Finance, IoT, Events, Metrics, Fraud, Grid
-- ==============================================================================
LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral;
SET client_min_messages = warning;

-- ---- teardown ---------------------------------------------------------------
DROP TABLE IF EXISTS trades           CASCADE;
DROP TABLE IF EXISTS portfolio        CASCADE;
DROP TABLE IF EXISTS iot_readings     CASCADE;
DROP TABLE IF EXISTS weather_stations        CASCADE;
DROP TABLE IF EXISTS user_events      CASCADE;
DROP TABLE IF EXISTS api_metrics      CASCADE;
DROP TABLE IF EXISTS fraud_signals    CASCADE;
DROP TABLE IF EXISTS order_pipeline        CASCADE;
DROP TABLE IF EXISTS energy_grid      CASCADE;

DELETE FROM spiral.metadata WHERE view_name NOT IN (
    SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'
);

SET client_min_messages = notice;

-- 1. FINANCE — trades
-- Aggregates: price (OHLCV), volume (SUM)
CREATE TABLE trades (
    t       timestamptz NOT NULL,
    symbol  text        NOT NULL,
    price   double precision,   -- Spiral: ohlcv
    volume  double precision,   -- Spiral: sum
    side    smallint
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'symbol');
INSERT INTO trades (t, symbol, price, volume, side)
SELECT '2025-05-30'::timestamptz + (n || ' minutes')::interval, 'AAPL', 185.0 + random(), 100 * random(), 1
FROM generate_series(1, 60) n;
SELECT spiral_refresh('trades');

-- 2. FINANCE — portfolio
-- Aggregates: market_value (STATS), pnl (SUM), cash (STATS)
CREATE TABLE portfolio (
    t            timestamptz NOT NULL,
    account_id   int         NOT NULL,
    asset        text        NOT NULL,
    market_value double precision,   -- Spiral: stats
    pnl          double precision,   -- Spiral: sum
    cash         double precision    -- Spiral: stats
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'account_id');
INSERT INTO portfolio (t, account_id, asset, market_value, pnl, cash)
SELECT '2025-05-30'::timestamptz + (n || ' hours')::interval, 1, 'AAPL', 50000 + random(), 100 * random(), 5000
FROM generate_series(1, 24) n;
SELECT spiral_refresh('portfolio');

-- 3. IoT — iot_readings
-- Aggregates: temp (OHLCV), humidity (STATS), battery (STATS)
CREATE TABLE iot_readings (
    t          timestamptz NOT NULL,
    device_id  int         NOT NULL,
    location   text        NOT NULL,
    temp       double precision,   -- Spiral: ohlcv
    humidity   double precision,   -- Spiral: stats
    pressure   double precision,
    battery    double precision    -- Spiral: stats
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'device_id');
INSERT INTO iot_readings (t, device_id, location, temp, humidity, pressure, battery)
SELECT '2025-05-30'::timestamptz + (n || ' minutes')::interval, 1, 'loc-A', 22.0 + random(), 50.0 + random(), 1013, 85
FROM generate_series(1, 60) n;
SELECT spiral_refresh('iot_readings');

-- 4. WEATHER — weather_stations
-- Aggregates: temp (OHLCV), wind_speed (STATS), precip_mm (SUM)
CREATE TABLE weather_stations (
    t          timestamptz NOT NULL,
    station_id int         NOT NULL,
    temp       double precision,   -- Spiral: ohlcv
    wind_speed double precision,   -- Spiral: stats
    precip_mm  double precision,   -- Spiral: sum
    uv_index   double precision,
    humidity   double precision
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'station_id');
INSERT INTO weather_stations (t, station_id, temp, wind_speed, precip_mm, uv_index, humidity)
SELECT '2025-05-30'::timestamptz + (n || ' hours')::interval, 1, 15.0 + random(), 5.0 + random(), 0, 5, 40
FROM generate_series(1, 24) n;
SELECT spiral_refresh('weather_stations');

-- 5. EVENTS — user_events
-- Aggregates: duration_ms (STATS)
CREATE TABLE user_events (
    t           timestamptz NOT NULL,
    user_id     int         NOT NULL,
    event_type  text        NOT NULL,
    page        text,
    duration_ms double precision,   -- Spiral: stats
    country     text
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'user_id');
INSERT INTO user_events (t, user_id, event_type, page, duration_ms, country)
SELECT '2025-05-30'::timestamptz + (random() * 60 || ' minutes')::interval, 1, 'click', '/home', 500, 'US'
FROM generate_series(1, 100) n;
SELECT spiral_refresh('user_events');

-- 6. METRICS — api_metrics
-- Aggregates: duration_ms (STATS), bytes_sent (SUM), error_count (SUM)
CREATE TABLE api_metrics (
    t           timestamptz NOT NULL,
    service     text        NOT NULL,
    status_code int         NOT NULL,
    duration_ms double precision,   -- Spiral: stats
    bytes_sent  double precision,   -- Spiral: sum
    error_count double precision    -- Spiral: sum
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'service');
INSERT INTO api_metrics (t, service, status_code, duration_ms, bytes_sent, error_count)
SELECT '2025-05-30'::timestamptz + (n || ' minutes')::interval, 'svc-A', 200, 25.0, 512, 0
FROM generate_series(1, 60) n;
SELECT spiral_refresh('api_metrics');

-- 7. SECURITY — fraud_signals
-- Aggregates: score (STATS), amount (SUM)
CREATE TABLE fraud_signals (
    t           timestamptz NOT NULL,
    account_id  int         NOT NULL,
    signal_type text        NOT NULL,
    score       double precision,   -- Spiral: stats
    amount      double precision,   -- Spiral: sum
    flagged     double precision
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'account_id');
INSERT INTO fraud_signals (t, account_id, signal_type, score, amount, flagged)
SELECT '2025-05-30'::timestamptz + (random() * 24 || ' hours')::interval, 1, 'velocity', 0.5, 100, 0
FROM generate_series(1, 100) n;
SELECT spiral_refresh('fraud_signals');

-- 8. OPS — order_pipeline
-- Aggregates: stage_ms (STATS), order_value (SUM)
CREATE TABLE order_pipeline (
    t           timestamptz NOT NULL,
    region      text        NOT NULL,
    status      text        NOT NULL,
    stage_ms    double precision,   -- Spiral: stats
    items       double precision,
    order_value double precision    -- Spiral: sum
) WITH (spiral.frames = '1m,1h,1d', spiral.tenant = 'region');
INSERT INTO order_pipeline (t, region, status, stage_ms, items, order_value)
SELECT '2025-05-30'::timestamptz + (n || ' minutes')::interval, 'reg-A', 'ok', 100, 1, 50
FROM generate_series(1, 60) n;
SELECT spiral_refresh('order_pipeline');

-- 9. ENERGY — energy_grid
-- Aggregates: load_mw (STATS), solar_mw (SUM), price_mwh (OHLCV)
CREATE TABLE energy_grid (
    t           timestamptz NOT NULL,
    zone_id     int         NOT NULL,
    load_mw     double precision,   -- Spiral: stats
    solar_mw    double precision,   -- Spiral: sum
    wind_mw     double precision,
    storage_mwh double precision,
    price_mwh   double precision    -- Spiral: ohlcv
) WITH (spiral.frames = '1h,1d', spiral.tenant = 'zone_id');
INSERT INTO energy_grid (t, zone_id, load_mw, solar_mw, wind_mw, storage_mwh, price_mwh)
SELECT '2025-05-30'::timestamptz + (n || ' hours')::interval, 1, 500, 400, 200, 1000, 45
FROM generate_series(1, 24) n;
SELECT spiral_refresh('energy_grid');

\echo '=== Summary ==='
SELECT m.view_name AS table_name, m.scope_columns AS tenant
FROM spiral.metadata m WHERE m.parent_view = 'BASE';
