# Spiral Benchmarks

This directory contains various benchmarks to evaluate Spiral's performance and features.

## Core Benchmarks
- **[core.sql](core.sql)**: Primary benchmark comparing Spiral vs. standard PostgreSQL for 1M rows. Focuses on ingestion and pre-aggregation.
- **[comprehensive.sql](comprehensive.sql)**: Validates all major features (IVM, Acceleration, Z-Order) with a 10M row dataset.
- **[scale.sql](scale.sql)**: Stress tests for 100M+ rows, evaluating bulk load and extreme scale acceleration.

## Specialized Benchmarks
- **[iot.sql](iot.sql)**: Simulates a TSBS-like IoT scenario with high-frequency telemetry.
- **[storage_optimization.sql](storage_optimization.sql)**: Evaluates storage reduction techniques (Time-as-Address).
- **[zorder_locality.sql](zorder_locality.sql)**: Tests multi-dimensional clustering and index locality.
- **[acceleration.sql](acceleration.sql)**: Deep dive into hierarchical query slicing performance.
- **[multi_dim.sql](multi_dim.sql)** / **[multi_dim_4d.sql](multi_dim_4d.sql)**: High-dimensional indexing tests.
- **[real_world.sql](real_world.sql)**: Tests with realistic, irregular time-series patterns.

## Running Benchmarks
Most benchmarks can be run via:
```bash
cargo pgrx run pg18 < benchmarks/core.sql
```
