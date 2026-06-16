-- Spiral vs Deltax vs Timescale vs Baseline ClickBench Queries

\timing on

-- Query 3: Basic aggregations
\echo '--- Query 3: Baseline (Raw Data) ---'
SET spiral.enable_planner_hook = false;
SELECT SUM(advengineid), COUNT(*), AVG(resolutionwidth) FROM hits;
SET spiral.enable_planner_hook = true;

\echo '--- Query 3: Spiral (Optimized Rollup) ---'
SELECT 
    spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) as sum_adv,
    spiral_stats_count_final(spiral_stats_merge(res_width_stats)) as count_all,
    spiral_stats_mean(spiral_stats_merge(res_width_stats)) as avg_res
FROM hits_1d_global;

\echo '--- Query 3: Deltax (Columnar) ---'
SELECT SUM(advengineid), COUNT(*), AVG(resolutionwidth) FROM hits_deltax;

\echo '--- Query 3: TimescaleDB (Columnar) ---'
SELECT SUM(advengineid), COUNT(*), AVG(resolutionwidth) FROM hits_timescale;

-- Query 10: Grouped aggregations
\echo '--- Query 10: Baseline (Raw Data) ---'
SET spiral.enable_planner_hook = false;
SELECT regionid, SUM(advengineid), COUNT(*) AS c, AVG(resolutionwidth)
FROM hits 
GROUP BY regionid 
ORDER BY c DESC LIMIT 10;
SET spiral.enable_planner_hook = true;

\echo '--- Query 10: Spiral (Rollup) ---'
SELECT 
    regionid, 
    spiral_stats_sum_final(spiral_stats_merge(adv_engine_stats)) as sum_adv,
    spiral_stats_count_final(spiral_stats_merge(res_width_stats)) as c,
    spiral_stats_mean(spiral_stats_merge(res_width_stats)) as avg_res
FROM hits_1d
GROUP BY regionid
ORDER BY c DESC LIMIT 10;

\echo '--- Query 10: Deltax (Columnar) ---'
SELECT regionid, SUM(advengineid), COUNT(*) AS c, AVG(resolutionwidth)
FROM hits_deltax 
GROUP BY regionid 
ORDER BY c DESC LIMIT 10;

\echo '--- Query 10: TimescaleDB (Columnar) ---'
SELECT regionid, SUM(advengineid), COUNT(*) AS c, AVG(resolutionwidth)
FROM hits_timescale 
GROUP BY regionid 
ORDER BY c DESC LIMIT 10;

\timing off
