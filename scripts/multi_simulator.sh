#!/bin/bash
# Spiral Multi-Table Simulator
# Feeds multiple tables with different tenant scales to demonstrate storage geometry.

DB_URL=${DATABASE_URL:-"postgres://localhost/postgres"}
KICKOFF=$(psql -At -d $DB_URL -c "SELECT COALESCE(current_setting('spiral.kickoff_date', true), '2000-01-01')::timestamptz")
echo "Starting simulator with kickoff: $KICKOFF"

# Create demonstration tables if they don't exist
psql -d $DB_URL <<EOF
-- High Cardinality Table (1024 tenants -> ~1s per page)
CREATE TABLE IF NOT EXISTS sensor_data (
    t timestamptz NOT NULL,
    sensor_id int NOT NULL,
    temperature double precision,
    humidity double precision
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'sensor_id',
    spiral.tenant_scale = 1024
);

-- Low Cardinality Table (128 tenants -> ~4s per page)
CREATE TABLE IF NOT EXISTS iot_metrics (
    t timestamptz NOT NULL,
    device_id int NOT NULL,
    battery_level double precision,
    signal_strength double precision
) WITH (
    spiral.frames = '1m,1h',
    spiral.tenant = 'device_id',
    spiral.tenant_scale = 128
);
EOF

while true; do
    NOW=$(date -u +"%Y-%m-%d %H:%M:%S")
    
    # Insert for sensor_data (1024 scale)
    # Generate 100 random sensors
    psql -d $DB_URL -c "INSERT INTO sensor_data (t, sensor_id, temperature, humidity) 
        SELECT '$NOW'::timestamptz, (random()*100)::int, random()*50, random()*100 
        FROM generate_series(1, 10);" > /dev/null 2>&1

    # Insert for iot_metrics (128 scale)
    # Generate 20 random devices
    psql -d $DB_URL -c "INSERT INTO iot_metrics (t, device_id, battery_level, signal_strength) 
        SELECT '$NOW'::timestamptz, (random()*20)::int, random()*100, -random()*100 
        FROM generate_series(1, 5);" > /dev/null 2>&1

    # Trigger refresh periodically
    if (( RANDOM % 10 == 0 )); then
        psql -d $DB_URL -c "SELECT spiral_refresh('sensor_data'); SELECT spiral_refresh('iot_metrics');" > /dev/null 2>&1
    fi

    sleep 1
done
