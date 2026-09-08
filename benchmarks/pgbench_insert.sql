-- A simple insert for pgbench to simulate multiple IoT sensors pushing data concurrently
INSERT INTO trigger_ticks (t, symbol_id, price, vol) 
VALUES (clock_timestamp(), (random()*10)::int, 60000 + random()*100, 10);
