# Spiral Development Guidelines

## Testing & Mathematical Integrity
Spiral is a mathematically sensitive project. All statistical algorithms and indexing logic must be verified against high-precision ground truths.

### Golden Reference Dataset
We maintain a "Golden Reference" in `tests/golden/` to prevent numerical regressions.
- **`values.csv`**: Raw input data for statistical tests.
- **`expected.json`**: Pre-calculated ground truth for Mean, Variance, Skewness, and Kurtosis.

#### Adding New Tests
When adding new statistical functions or modifying existing ones:
1.  **Update the Golden Generator**: Modify `src/bin/generate_golden.rs` to include new expectations or different data distributions.
2.  **Regenerate**: Run `cargo run --bin generate_golden` to update the files in `tests/golden/`.
3.  **Implement Rust Assertions**: Add `#[pg_test]` cases in `src/lib.rs` that load these golden files.
4.  **Implement SQL Assertions**: Add a corresponding `.sql` test in `tests/pg_regress/sql/` and update the `.out` file.

### Critical Logic Paths
Always verify the following when making changes:
- **Timestamp Mapping**: Ensure `spiral(t)` remains consistent across timezones.
- **Z-Order Interleaving**: Bit-placement in `spiral_zorder` must remain stable to preserve index compatibility.
- **Parallel Merging**: Verify that `merge` operations are associative and numerically stable.

## Workflow
- **Precision First**: Use `epsilon` (typically `1e-9`) for all floating-point assertions.
- **Regression Testing**: NEVER commit a change to statistical logic without a corresponding golden test.
