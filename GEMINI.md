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

## Architectural Constraints
- **Event Streaming (No pg_notify)**: NEVER use `pg_notify` for high-throughput tracking (e.g., changelogs). It causes exclusive SLRU locks on the global queue, degrading performance at scale. Always use **Logical Decoding** (via `pg_logical_slot_get_changes` or similar plugins) managed natively within the `pgrx` extension.

## pgrx Testing Caveats
- **Transaction Wrappers**: `cargo pgrx test` executes each `#[pg_test]` inside a single transaction that is eventually rolled back. 
- **Logical Decoding Tests**: Because logical decoding only reads *committed* WAL changes, `pg_logical_slot_get_changes()` will not see `INSERT`s performed within the same `#[pg_test]`. Tests involving decoding should serve as smoke-tests (e.g., verifying slot creation and query execution) unless using separate connections.

## AI Assistant Guidelines
- **Autonomy**: Do not ask for permission to run terminal commands (like `cargo`, `ps`, `ls`) or to edit files. Execute tasks directly and proactively.
- **File Editing**: NEVER use `cat << EOF` or bash redirection to create or modify files. ALWAYS use the specialized `write_to_file` and `replace_file_content` tools to avoid terminal hangs.
