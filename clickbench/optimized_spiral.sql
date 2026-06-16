-- Create a very compact rollup for Query 3
DROP TABLE IF EXISTS hits_1d_global CASCADE;

CREATE TABLE hits_1d_global AS 
SELECT 
    to_timestamp(((spiral(t) / 86400) * 86400)::double precision) as t,
    spiral_stats(advengineid) as adv_engine_stats,
    spiral_stats(resolutionwidth) as res_width_stats
FROM hits
GROUP BY 1;

-- This table should only have ~30 rows (one per day)
SELECT COUNT(*) FROM hits_1d_global;

\timing on
\echo '--- Query 3: Spiral (Optimized Rollup) ---'
SELECT 
    spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) as sum_adv,
    spiral_stats_count_final(spiral_stats_merge(res_width_stats)) as count_all,
    spiral_stats_mean(spiral_stats_merge(res_width_stats)) as avg_res
FROM hits_1d_global;
\timing off
