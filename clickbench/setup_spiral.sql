-- ClickBench setup for Spiral
CREATE EXTENSION IF NOT EXISTS spiral;

DROP TABLE IF EXISTS hits CASCADE;
DROP TABLE IF EXISTS hits_1d CASCADE;

-- 1. Create standard hits table
\i clickbench/scripts/create.sql

-- 2. Load data
COPY hits FROM '/Users/jonatas/code/spiral/clickbench/hits.csv' WITH (FORMAT csv, DELIMITER E'\t');

-- 3. Define aggregations for spiral rollups
DELETE FROM spiral.sources WHERE view_name = 'hits_1d';
INSERT INTO spiral.sources (view_name, base_view, frame_seconds, base_column, formula, mat_column)
VALUES 
('hits_1d', 'hits', 86400, 'advengineid', 'stats', 'adv_engine_stats'),
('hits_1d', 'hits', 86400, 'resolutionwidth', 'stats', 'res_width_stats'),
('hits_1d', 'hits', 86400, 'userid', 'sketch', 'user_sketch');

-- 4. Create the table manually to avoid derive_child_sql auto-summing text columns
CREATE TABLE hits_1d (
    t timestamptz,
    regionid int,
    advengineid int,
    adv_engine_stats bytea,
    res_width_stats bytea,
    user_sketch bytea
);

-- 5. Register the view
SELECT spiral_register_view('hits_1d', 'BASE', 86400, 'hits', ARRAY['regionid', 'advengineid']);

-- 6. Populating it once:
INSERT INTO hits_1d (t, regionid, advengineid, adv_engine_stats, res_width_stats, user_sketch)
SELECT 
    to_timestamp(((spiral(t) / 86400) * 86400)::double precision) as t,
    regionid,
    advengineid,
    spiral_stats(advengineid) as adv_engine_stats,
    spiral_stats(resolutionwidth) as res_width_stats,
    spiral_sketch(userid) as user_sketch
FROM hits
GROUP BY 1, 2, 3;
