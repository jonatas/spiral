-- Spiral vs Baseline ClickBench Queries

\timing on

-- Query 3: Basic aggregations
\echo '--- Query 3: Baseline (Raw Data) ---'
SELECT SUM(advengineid), COUNT(*), AVG(resolutionwidth) FROM hits;

\echo '--- Query 3: Spiral (Rollup) ---'
SELECT 
    spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) as sum_adv,
    spiral_stats_count_final(spiral_stats_merge(res_width_stats)) as count_all,
    spiral_stats_mean(spiral_stats_merge(res_width_stats)) as avg_res
FROM hits_1d;


-- Query 10: Grouped aggregations with Count Distinct
\echo '--- Query 10: Baseline (Raw Data) ---'
SELECT regionid, SUM(advengineid), COUNT(*) AS c, AVG(resolutionwidth), COUNT(DISTINCT userid) 
FROM hits 
GROUP BY regionid 
ORDER BY c DESC LIMIT 10;

\echo '--- Query 10: Spiral (Rollup) ---'
-- Note: We can only use what's in the rollup (regionid, advengineid are in grouping)
-- To get results by regionid only, we must merge.
SELECT 
    regionid, 
    spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) as sum_adv,
    spiral_stats_count_final(spiral_stats_merge(res_width_stats)) as c,
    spiral_stats_mean(spiral_stats_merge(res_width_stats)) as avg_res
FROM hits_1d
GROUP BY regionid
ORDER BY c DESC LIMIT 10;

\timing off
