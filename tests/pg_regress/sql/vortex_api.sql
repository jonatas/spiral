LOAD 'spiral';
SET spiral.kickoff_date = '2026-05-27 10:00:00Z';
SELECT current_setting('spiral.kickoff_date');
SELECT spiral_get_storage_stats('demo_storage'::regclass::oid::int) -> 'kickoff_epoch';
