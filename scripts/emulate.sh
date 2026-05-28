#!/usr/bin/env bash
# Real-time data emulator for multi_domain_demo_light
# Calls psql once per tick (2s) so it stays interruptible with Ctrl+C.
# Usage: scripts/emulate.sh [tick_count]  (default: infinite)

set -euo pipefail

PSQL="${PSQL_BIN:-/opt/homebrew/opt/postgresql@18/bin/psql}"
CONN="${DATABASE_URL:-}"
if [[ -z "$CONN" ]]; then
  PSQL_ARGS=(-h "$HOME/.pgrx" -p 28818 -U jonatas postgres)
else
  PSQL_ARGS=("$CONN")
fi

MAX="${1:-0}"   # 0 = infinite
tick=0

emit_tick() {
  "$PSQL" "${PSQL_ARGS[@]}" -q 2>&1 <<'ENDSQL'
LOAD 'spiral';
DO $$
DECLARE
    now_t timestamptz := now();
    last_price float8; last_volume float8;
    last_mktval float8; last_pnl float8; last_cash float8;
    last_temp float8; last_humidity float8; last_battery float8;
    last_wx_temp float8; last_wind float8;
    last_duration float8;
    last_api_dur float8; last_bytes float8;
    last_score float8; last_amount float8;
    last_stage_ms float8; last_order_val float8;
    last_load float8; last_solar float8; last_price_mwh float8;
BEGIN
    -- 1. TRADES (AAPL)
    SELECT COALESCE(price,185.0), COALESCE(volume,100.0)
    INTO last_price, last_volume
    FROM trades WHERE symbol='AAPL' ORDER BY t DESC LIMIT 1;
    INSERT INTO trades(t,symbol,price,volume,side) VALUES(
        now_t,'AAPL',
        GREATEST(1, last_price + (random()-0.5)*2.0),
        GREATEST(1, last_volume*(0.8+random()*0.4)),
        CASE WHEN random()>0.5 THEN 1 ELSE -1 END);

    -- 2. PORTFOLIO (account_id=1)
    SELECT COALESCE(market_value,50000.0), COALESCE(pnl,0.0), COALESCE(cash,5000.0)
    INTO last_mktval, last_pnl, last_cash
    FROM portfolio WHERE account_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO portfolio(t,account_id,asset,market_value,pnl,cash) VALUES(
        now_t,1,'AAPL',
        GREATEST(0, last_mktval*(1+(random()-0.5)*0.01)),
        last_pnl+(random()-0.48)*50,
        GREATEST(0, last_cash+(random()-0.5)*100));

    -- 3. IoT READINGS (device_id=1)
    SELECT COALESCE(temp,22.0), COALESCE(humidity,50.0), COALESCE(battery,85.0)
    INTO last_temp, last_humidity, last_battery
    FROM iot_readings WHERE device_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO iot_readings(t,device_id,location,temp,humidity,pressure,battery) VALUES(
        now_t,1,'loc-A',
        last_temp+(random()-0.5)*0.5,
        LEAST(100,GREATEST(0,last_humidity+(random()-0.5)*2)),
        1013+(random()-0.5)*3,
        LEAST(100,GREATEST(0,last_battery-random()*0.02)));

    -- 4. WEATHER STATIONS (station_id=1)
    SELECT COALESCE(temp,15.0), COALESCE(wind_speed,5.0)
    INTO last_wx_temp, last_wind
    FROM weather_stations WHERE station_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO weather_stations(t,station_id,temp,wind_speed,precip_mm,uv_index,humidity) VALUES(
        now_t,1,
        last_wx_temp+(random()-0.5)*0.3,
        GREATEST(0,last_wind+(random()-0.5)*1.0),
        GREATEST(0,CASE WHEN random()>0.9 THEN random()*5 ELSE 0 END),
        LEAST(11,GREATEST(0,5+(random()-0.5)*2)),
        LEAST(100,GREATEST(0,40+(random()-0.5)*5)));

    -- 5. USER EVENTS (user_id=1)
    SELECT COALESCE(duration_ms,500.0) INTO last_duration
    FROM user_events WHERE user_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO user_events(t,user_id,event_type,page,duration_ms,country) VALUES(
        now_t,1,
        (ARRAY['click','scroll','view','submit'])[1+floor(random()*4)::int],
        (ARRAY['/home','/dashboard','/settings','/profile'])[1+floor(random()*4)::int],
        GREATEST(10,last_duration*(0.7+random()*0.6)),
        (ARRAY['US','BR','DE','JP','FR'])[1+floor(random()*5)::int]);

    -- 6. API METRICS (service='svc-A')
    SELECT COALESCE(duration_ms,25.0), COALESCE(bytes_sent,512.0)
    INTO last_api_dur, last_bytes
    FROM api_metrics WHERE service='svc-A' ORDER BY t DESC LIMIT 1;
    INSERT INTO api_metrics(t,service,status_code,duration_ms,bytes_sent,error_count) VALUES(
        now_t,'svc-A',
        CASE WHEN random()>0.97 THEN 500 WHEN random()>0.93 THEN 429 ELSE 200 END,
        GREATEST(1,last_api_dur*(0.5+random())),
        GREATEST(0,last_bytes*(0.8+random()*0.4)),
        CASE WHEN random()>0.95 THEN 1 ELSE 0 END);

    -- 7. FRAUD SIGNALS (account_id=1)
    SELECT COALESCE(score,0.5), COALESCE(amount,100.0)
    INTO last_score, last_amount
    FROM fraud_signals WHERE account_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO fraud_signals(t,account_id,signal_type,score,amount,flagged) VALUES(
        now_t,1,
        (ARRAY['velocity','geo_mismatch','pattern','device'])[1+floor(random()*4)::int],
        LEAST(1,GREATEST(0,last_score+(random()-0.5)*0.1)),
        GREATEST(0,last_amount*(0.5+random())),
        CASE WHEN last_score>0.8 THEN 1 ELSE 0 END);

    -- 8. ORDER PIPELINE (region='reg-A')
    SELECT COALESCE(stage_ms,100.0), COALESCE(order_value,50.0)
    INTO last_stage_ms, last_order_val
    FROM order_pipeline WHERE region='reg-A' ORDER BY t DESC LIMIT 1;
    INSERT INTO order_pipeline(t,region,status,stage_ms,items,order_value) VALUES(
        now_t,'reg-A',
        (ARRAY['ok','pending','failed'])[1+floor(random()*2.99)::int],
        GREATEST(1,last_stage_ms*(0.5+random())),
        1+floor(random()*5)::int,
        GREATEST(1,last_order_val*(0.8+random()*0.4)));

    -- 9. ENERGY GRID (zone_id=1)
    SELECT COALESCE(load_mw,500.0), COALESCE(solar_mw,400.0), COALESCE(price_mwh,45.0)
    INTO last_load, last_solar, last_price_mwh
    FROM energy_grid WHERE zone_id=1 ORDER BY t DESC LIMIT 1;
    INSERT INTO energy_grid(t,zone_id,load_mw,solar_mw,wind_mw,storage_mwh,price_mwh) VALUES(
        now_t,1,
        GREATEST(0,last_load+(random()-0.5)*20),
        GREATEST(0,last_solar+(random()-0.5)*30),
        GREATEST(0,200+(random()-0.5)*50),
        GREATEST(0,1000+(random()-0.5)*100),
        GREATEST(0,last_price_mwh+(random()-0.5)*3));

END;
$$;
ENDSQL
}

echo "emulator starting (interval=2s, max=${MAX:-∞})"
while true; do
  tick=$((tick+1))
  emit_tick && echo "tick $tick @ $(date +%H:%M:%S)" || echo "tick $tick FAILED"
  [[ "$MAX" -gt 0 && "$tick" -ge "$MAX" ]] && break
  sleep 2
done
