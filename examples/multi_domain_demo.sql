-- ==============================================================================
-- Spiral Multi-Domain Demo: Finance, IoT, Events, Metrics, Fraud, Grid
-- ==============================================================================
-- Loads ~3 months of realistic time-series data across 9 domains so you can
-- explore query acceleration, dirty-bucket tracking, and rollup hierarchies
-- with real-world cardinality and access patterns.
--
-- Usage:
--   psql $DATABASE_URL -f examples/multi_domain_demo.sql
--
-- Estimated load time: 5–15 minutes depending on hardware.
-- ==============================================================================

LOAD 'spiral';
CREATE EXTENSION IF NOT EXISTS spiral;
SET client_min_messages = warning;

-- ---- teardown ---------------------------------------------------------------
DROP TABLE IF EXISTS trades           CASCADE;
DROP TABLE IF EXISTS trades_1m        CASCADE;
DROP TABLE IF EXISTS trades_1h        CASCADE;
DROP TABLE IF EXISTS trades_1d        CASCADE;
DROP TABLE IF EXISTS portfolio        CASCADE;
DROP TABLE IF EXISTS portfolio_1h     CASCADE;
DROP TABLE IF EXISTS portfolio_1d     CASCADE;
DROP TABLE IF EXISTS iot_readings     CASCADE;
DROP TABLE IF EXISTS iot_readings_1m  CASCADE;
DROP TABLE IF EXISTS iot_readings_1h  CASCADE;
DROP TABLE IF EXISTS iot_readings_1d  CASCADE;
DROP TABLE IF EXISTS weather_stations        CASCADE;
DROP TABLE IF EXISTS weather_stations_1h     CASCADE;
DROP TABLE IF EXISTS weather_stations_1d     CASCADE;
DROP TABLE IF EXISTS user_events      CASCADE;
DROP TABLE IF EXISTS user_events_1m   CASCADE;
DROP TABLE IF EXISTS user_events_1h   CASCADE;
DROP TABLE IF EXISTS user_events_1d   CASCADE;
DROP TABLE IF EXISTS api_metrics      CASCADE;
DROP TABLE IF EXISTS api_metrics_1m   CASCADE;
DROP TABLE IF EXISTS api_metrics_1h   CASCADE;
DROP TABLE IF EXISTS api_metrics_1d   CASCADE;
DROP TABLE IF EXISTS fraud_signals    CASCADE;
DROP TABLE IF EXISTS fraud_signals_1h CASCADE;
DROP TABLE IF EXISTS fraud_signals_1d CASCADE;
DROP TABLE IF EXISTS order_pipeline        CASCADE;
DROP TABLE IF EXISTS order_pipeline_1m     CASCADE;
DROP TABLE IF EXISTS order_pipeline_1h     CASCADE;
DROP TABLE IF EXISTS order_pipeline_1d     CASCADE;
DROP TABLE IF EXISTS energy_grid      CASCADE;
DROP TABLE IF EXISTS energy_grid_1h   CASCADE;
DROP TABLE IF EXISTS energy_grid_1d   CASCADE;
DROP TABLE IF EXISTS sensor_data      CASCADE;
DROP TABLE IF EXISTS sensor_data_1m   CASCADE;
DROP TABLE IF EXISTS sensor_data_1h   CASCADE;
DROP TABLE IF EXISTS sensor_data_1d   CASCADE;
DROP TABLE IF EXISTS legacy_metrics   CASCADE;
DROP TABLE IF EXISTS legacy_metrics_1m CASCADE;
DROP TABLE IF EXISTS legacy_metrics_1h CASCADE;

DELETE FROM spiral.metadata WHERE view_name NOT IN (
    SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'
);

SET client_min_messages = notice;

-- ==============================================================================
-- 1. FINANCE — trades
--    Exchange tick data: 5 symbols × 90 days of 30-second ticks (sampled ~40%)
--    Aggregation: price as OHLCV candles, volume summed
-- ==============================================================================
\echo '=== 1/9  trades ==='

CREATE TABLE trades (
    t       timestamptz NOT NULL,
    symbol  text        NOT NULL,
    price   double precision,   -- Spiral: ohlcv
    volume  double precision,   -- Spiral: sum
    side    smallint            -- 1=buy  -1=sell
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'symbol'
);

INSERT INTO trades (t, symbol, price, volume, side)
SELECT
    ts,
    sym,
    base_price
        * (1 + 0.002 * sin(extract(epoch from ts) / 3600.0)
             + 0.001 * (random() - 0.5)
             + drift * extract(day from ts - '2025-03-01'::date) / 90.0),
    (100 + 900 * random())
        * CASE WHEN extract(hour from ts) BETWEEN 9 AND 16 THEN 3.0 ELSE 0.5 END,
    CASE WHEN random() > 0.5 THEN 1 ELSE -1 END
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:59'::timestamptz,
                    '30 seconds'::interval) ts,
    (VALUES
        ('AAPL',  185.0,  0.05),
        ('MSFT',  410.0,  0.04),
        ('NVDA',  875.0,  0.08),
        ('BTC',  62000.0, 0.12),
        ('ETH',   3400.0, 0.10)
    ) AS syms(sym, base_price, drift)
WHERE random() < 0.4;

SELECT spiral_refresh('trades');

-- ==============================================================================
-- 2. FINANCE — portfolio
--    Hourly NAV snapshots: 20 accounts × 5 assets × 90 days
--    Good for P&L trending and account-level stats queries
-- ==============================================================================
\echo '=== 2/9  portfolio ==='

CREATE TABLE portfolio (
    t            timestamptz NOT NULL,
    account_id   int         NOT NULL,
    asset        text        NOT NULL,
    market_value double precision,   -- Spiral: stats
    pnl          double precision,   -- Spiral: sum
    cash         double precision    -- Spiral: stats
) WITH (
    spiral.frames = '1h,1d',
    spiral.tenant = 'account_id'
);

INSERT INTO portfolio (t, account_id, asset, market_value, pnl, cash)
SELECT
    ts,
    acct,
    sym,
    seed * (1 + 0.001 * (random() - 0.5) * 24
              + 0.0003 * extract(epoch from (ts - '2025-03-01'::timestamptz)) / 3600),
    seed * 0.15 * (random() - 0.3),
    5000 + 20000 * random()
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:00'::timestamptz,
                    '1 hour'::interval) ts,
    generate_series(1, 20) acct,
    (VALUES ('AAPL', 50000), ('MSFT', 80000), ('NVDA', 30000),
            ('BTC', 120000), ('CASH', 10000)) AS a(sym, seed);

SELECT spiral_refresh('portfolio');

-- ==============================================================================
-- 3. IoT — iot_readings
--    100 devices across 5 locations, 10-second readings (sampled ~20%), 30 days
--    Demonstrates high-cardinality tenants and battery drain patterns
-- ==============================================================================
\echo '=== 3/9  iot_readings ==='

CREATE TABLE iot_readings (
    t          timestamptz NOT NULL,
    device_id  int         NOT NULL,
    location   text        NOT NULL,
    temp       double precision,   -- Spiral: ohlcv
    humidity   double precision,   -- Spiral: stats
    pressure   double precision,   -- Spiral: stats
    battery    double precision    -- Spiral: stats
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'device_id'
);

INSERT INTO iot_readings (t, device_id, location, temp, humidity, pressure, battery)
SELECT
    ts,
    dev,
    locs.loc,
    base_t + 5 * sin(2 * pi() * extract(hour from ts) / 24.0) + 0.5 * (random() - 0.5),
    base_h + 10 * sin(2 * pi() * (extract(hour from ts) - 6) / 24.0) + 2 * (random() - 0.5),
    1013 + 5 * sin(extract(epoch from ts) / 86400.0) + random(),
    GREATEST(10, LEAST(100,
        85 - 0.01 * extract(epoch from (ts - '2025-05-01'::timestamptz)) / 60.0 + random() * 2))
FROM
    generate_series('2025-05-01 00:00'::timestamptz,
                    '2025-05-31 23:59'::timestamptz,
                    '10 seconds'::interval) ts,
    generate_series(1, 100) dev,
    LATERAL (SELECT
        CASE (dev % 5)
            WHEN 0 THEN 'warehouse-A'
            WHEN 1 THEN 'office-B'
            WHEN 2 THEN 'cold-room'
            WHEN 3 THEN 'roof-C'
            ELSE        'server-room'
        END AS loc,
        CASE (dev % 5)
            WHEN 2 THEN  4.0   -- cold room
            WHEN 4 THEN 28.0   -- server room
            ELSE        22.0
        END AS base_t,
        CASE (dev % 5)
            WHEN 2 THEN 85.0   -- cold room: high humidity
            ELSE        55.0
        END AS base_h
    ) locs
WHERE random() < 0.2;

SELECT spiral_refresh('iot_readings');

-- ==============================================================================
-- 4. WEATHER — weather_stations
--    10 stations, 5-minute readings, 90 days — seasonal + diurnal cycles
-- ==============================================================================
\echo '=== 4/9  weather_stations ==='

CREATE TABLE weather_stations (
    t          timestamptz NOT NULL,
    station_id int         NOT NULL,
    temp       double precision,   -- Spiral: ohlcv
    wind_speed double precision,   -- Spiral: stats
    precip_mm  double precision,   -- Spiral: sum
    uv_index   double precision,   -- Spiral: stats
    humidity   double precision    -- Spiral: stats
) WITH (
    spiral.frames = '1h,1d',
    spiral.tenant = 'station_id'
);

INSERT INTO weather_stations (t, station_id, temp, wind_speed, precip_mm, uv_index, humidity)
SELECT
    ts,
    st,
    15 + 10 * sin(2 * pi() * (extract(doy from ts) / 365.0 - 0.2))
       +  8 * sin(2 * pi() * extract(hour from ts) / 24.0)
       + (st * 0.8 - 4)
       + 1.5 * (random() - 0.5),
    GREATEST(0, 5 + 10 * random() + 8 * sin(extract(epoch from ts) / 7200.0)),
    CASE WHEN random() < 0.05 THEN random() * 8 ELSE 0 END,
    GREATEST(0, CASE WHEN extract(hour from ts) BETWEEN 6 AND 18
                     THEN 6 * sin(pi() * (extract(hour from ts) - 6) / 12.0) * (1 - 0.5 * random())
                     ELSE 0 END),
    40 + 30 * random() + 15 * sin(2 * pi() * extract(doy from ts) / 365.0)
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:55'::timestamptz,
                    '5 minutes'::interval) ts,
    generate_series(1, 10) st;

SELECT spiral_refresh('weather_stations');

-- ==============================================================================
-- 5. EVENTS — user_events
--    2M click/page events across 10k users, random times over 90 days
--    Tenant = user_id; shows per-user session duration distribution
-- ==============================================================================
\echo '=== 5/9  user_events ==='

CREATE TABLE user_events (
    t           timestamptz NOT NULL,
    user_id     int         NOT NULL,
    event_type  text        NOT NULL,
    page        text,
    duration_ms double precision,   -- Spiral: stats
    country     text
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'user_id'
);

INSERT INTO user_events (t, user_id, event_type, page, duration_ms, country)
SELECT
    '2025-03-01'::timestamptz
        + (random() * 91  || ' days')::interval
        + (random() * 86400 || ' seconds')::interval,
    (random() * 10000)::int + 1,
    (ARRAY['pageview','click','scroll','purchase',
           'signup','logout','search'])[1 + (random() * 6)::int],
    (ARRAY['/home','/','/pricing','/docs','/blog',
           '/login','/dashboard','/checkout',
           '/search','/about'])[1 + (random() * 9)::int],
    GREATEST(50, 200 + 5000 * random()),
    (ARRAY['US','GB','DE','FR','BR','IN','JP','CA','AU','MX'])[1 + (random() * 9)::int]
FROM generate_series(1, 2000000) n;

SELECT spiral_refresh('user_events');

-- ==============================================================================
-- 6. METRICS — api_metrics
--    6 services × 1-minute resolution × 90 days; latency + error rate
-- ==============================================================================
\echo '=== 6/9  api_metrics ==='

CREATE TABLE api_metrics (
    t           timestamptz NOT NULL,
    service     text        NOT NULL,
    status_code int         NOT NULL,
    duration_ms double precision,   -- Spiral: stats
    bytes_sent  double precision,   -- Spiral: sum
    error_count double precision    -- Spiral: sum
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'service'
);

INSERT INTO api_metrics (t, service, status_code, duration_ms, bytes_sent, error_count)
SELECT
    ts,
    svc,
    CASE WHEN r < 0.92 THEN 200
         WHEN r < 0.96 THEN 400
         WHEN r < 0.98 THEN 500
         ELSE 503 END,
    GREATEST(1, base_latency
                * (1 + 0.5 * sin(2 * pi() * extract(hour from ts) / 24.0))
                + 50 * random()),
    512 + 50000 * random(),
    CASE WHEN r > 0.96 THEN 1 ELSE 0 END
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:59'::timestamptz,
                    '1 minute'::interval) ts,
    (VALUES
        ('auth-service',    35.0),
        ('payment-api',    180.0),
        ('user-api',        25.0),
        ('search-api',     120.0),
        ('notify-service',  15.0),
        ('analytics-api',   90.0)
    ) AS svcs(svc, base_latency),
    LATERAL (SELECT random() AS r) rnd;

SELECT spiral_refresh('api_metrics');

-- ==============================================================================
-- 7. SECURITY — fraud_signals
--    500k signal events across 50k accounts — score distribution skewed toward
--    high-risk cohort; flagged ratio ~2 %
-- ==============================================================================
\echo '=== 7/9  fraud_signals ==='

CREATE TABLE fraud_signals (
    t           timestamptz NOT NULL,
    account_id  int         NOT NULL,
    signal_type text        NOT NULL,
    score       double precision,   -- Spiral: stats
    amount      double precision,   -- Spiral: sum
    flagged     double precision    -- Spiral: sum  (1.0 / 0.0)
) WITH (
    spiral.frames = '1h,1d',
    spiral.tenant = 'account_id'
);

INSERT INTO fraud_signals (t, account_id, signal_type, score, amount, flagged)
SELECT
    '2025-03-01'::timestamptz
        + (random() * 91    || ' days')::interval
        + (random() * 86400 || ' seconds')::interval,
    (random() * 50000)::int + 1,
    (ARRAY['velocity','geo_anomaly','device_change',
           'card_test','unusual_merchant','night_txn'])[1 + (random() * 5)::int],
    CASE WHEN (random() * 50000)::int < 500   -- 1 % high-risk accounts
         THEN 0.7 + 0.3 * random()
         ELSE random() * 0.4 END,
    CASE WHEN random() < 0.8 THEN 10 + 490 * random()
         ELSE 1000 + 9000 * random() END,
    CASE WHEN random() < 0.02 THEN 1.0 ELSE 0.0 END
FROM generate_series(1, 500000) n;

SELECT spiral_refresh('fraud_signals');

-- ==============================================================================
-- 8. OPS — order_pipeline
--    5 regions × 30-second arrivals (sampled 30%) × 90 days
--    stage_ms shows latency per pipeline stage; value shows GMV
-- ==============================================================================
\echo '=== 8/9  order_pipeline ==='

CREATE TABLE order_pipeline (
    t           timestamptz NOT NULL,
    region      text        NOT NULL,
    status      text        NOT NULL,
    stage_ms    double precision,   -- Spiral: stats
    items       double precision,   -- Spiral: sum
    order_value double precision    -- Spiral: sum
) WITH (
    spiral.frames = '1m,1h,1d',
    spiral.tenant = 'region'
);

INSERT INTO order_pipeline (t, region, status, stage_ms, items, order_value)
SELECT
    ts,
    reg,
    (ARRAY['received','validated','payment',
           'picking','shipped','delivered','returned'])[1 + (random() * 6)::int],
    GREATEST(10, base_ms + base_ms * random()),
    1 + (random() * 15)::int,
    CASE WHEN random() < 0.70 THEN  20 +  200 * random()
         WHEN random() < 0.95 THEN 200 +  800 * random()
         ELSE                       1000 + 4000 * random() END
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:59'::timestamptz,
                    '30 seconds'::interval) ts,
    (VALUES
        ('us-east',  120.0),
        ('eu-west',  180.0),
        ('ap-south', 250.0),
        ('us-west',  100.0),
        ('sa-east',  200.0)
    ) AS regions(reg, base_ms)
WHERE random() < 0.3;

SELECT spiral_refresh('order_pipeline');

-- ==============================================================================
-- 9. ENERGY — energy_grid
--    8 zones, 5-minute telemetry, 90 days
--    Solar/wind generation + storage + spot price — classic multi-variate grid
-- ==============================================================================
\echo '=== 9/9  energy_grid ==='

CREATE TABLE energy_grid (
    t           timestamptz NOT NULL,
    zone_id     int         NOT NULL,
    load_mw     double precision,   -- Spiral: stats
    solar_mw    double precision,   -- Spiral: sum
    wind_mw     double precision,   -- Spiral: sum
    storage_mwh double precision,   -- Spiral: stats
    price_mwh   double precision    -- Spiral: ohlcv
) WITH (
    spiral.frames = '1h,1d',
    spiral.tenant = 'zone_id'
);

INSERT INTO energy_grid (t, zone_id, load_mw, solar_mw, wind_mw, storage_mwh, price_mwh)
SELECT
    ts,
    z,
    500 + 300 * sin(2 * pi() * (extract(hour from ts) - 8) / 24.0) + 50 * random() + z * 20,
    GREATEST(0,
        400 * sin(pi() * (extract(hour from ts) - 6) / 12.0) * (1 + 0.1 * random()))
        * CASE WHEN extract(hour from ts) BETWEEN 6 AND 20 THEN 1 ELSE 0 END,
    GREATEST(0, 200 + 150 * sin(extract(epoch from ts) / 43200.0) + 80 * (random() - 0.5)),
    GREATEST(0, 1000 - 50 * (z - 1) + 100 * sin(extract(epoch from ts) / 3600.0) + 30 * random()),
    GREATEST(10, 45 + 30 * sin(2 * pi() * (extract(hour from ts) - 17) / 24.0) + 15 * random())
FROM
    generate_series('2025-03-01 00:00'::timestamptz,
                    '2025-05-31 23:55'::timestamptz,
                    '5 minutes'::interval) ts,
    generate_series(1, 8) z;

SELECT spiral_refresh('energy_grid');

-- ==============================================================================
-- Summary
-- ==============================================================================
\echo '=== Summary ==='
SELECT
    m.view_name                                                      AS table_name,
    m.scope_columns                                                  AS tenant,
    pg_size_pretty(pg_total_relation_size(m.view_name::regclass))   AS size,
    (SELECT COUNT(*) FROM information_schema.columns
     WHERE table_name = m.view_name AND table_schema = 'public')     AS cols
FROM spiral.metadata m
WHERE m.parent_view = 'BASE'
ORDER BY pg_total_relation_size(m.view_name::regclass) DESC;
