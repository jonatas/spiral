# Follow-Up Issues

These issue drafts are intended to keep the implementation, docs, and benchmark claims aligned. Each item is written to be pasted directly into GitHub as a new issue.

## 1. Tighten planner rewrite correctness for aggregate mapping [PARTIALLY DONE]

**Status Update (2026-05-25):** 
- Implemented support for complex aggregates (Stats, T-Digest, Sketch) with proper type alignment.
- Added recursive aggregate rewriter to support nested functions like `spiral_stats_mean(spiral_stats(val))`.
- Fixed `ohlcv` mapping to correctly route `MIN`/`MAX` to `_l`/`_h` sub-columns.

**Title**
`planner: validate aggregate mapping correctness across rollup and raw fallback segments`

**Description**
The planner hook currently rewrites eligible queries by replacing a base-table RTE with a subquery that unions rollup tiers and raw fallback segments.

We need a correctness audit for aggregate mapping, especially around:

- `AVG` semantics across raw and rollup segments
- `COUNT`, `MIN`, and `MAX` behavior when sourced from materialized columns
- mixed raw/rollup unions with grouping columns
- unsupported or partially supported aggregate functions

The goal is to make the planner's supported surface explicit and testable instead of relying on optimistic fallback assumptions.

**Acceptance criteria**

- Define the supported aggregate matrix in code comments and docs
- Add regression coverage for `SUM`, `COUNT`, `MIN`, `MAX`, and `AVG`
- Ensure unsupported aggregates always fall back cleanly
- Document any cases where exact rewrites are intentionally not attempted

**Relevant files**

- `src/hooks.rs`
- `src/rollup.rs`
- `docs/ARCHITECTURAL_GUIDE.md`
- `docs/QUERY_ACCELERATION.md`

## 2. Formalize sketch/quantile semantics and error bounds

**Title**
`stats: document and verify sketch semantics used for quantile rollups`

**Description**
The current `spiral_sketch_*` implementation uses a compact centroid-based merge strategy, but the docs currently read closer to a full t-digest accuracy claim than the code justifies.

We need to decide whether this structure is:

- a deliberately simple experimental sketch with documented limits, or
- intended to approximate t-digest closely enough for stronger claims

Either way, the implementation needs explicit semantics, test coverage, and benchmark/error characterization.

**Acceptance criteria**

- Document the exact sketch algorithm and its intended guarantees
- Add tests for merge stability and quantile estimation behavior
- Record error characteristics for representative distributions
- Update docs so they do not imply stronger mathematical guarantees than the implementation provides

**Relevant files**

- `src/stats.rs`
- `docs/BENCHMARK.md`
- `README.md`

## 3. Complete or narrow the TAM implementation surface [PARTIALLY DONE]

**Status Update (2026-05-25):**
- Implemented `tuple_insert` for functional basic inserts.
- Added basic `ANALYZE` support via `relation_estimate_size`.
- Updated `docs/TAM_AUDIT.md` with current status.

**Title**
`tam: audit unimplemented callbacks and define supported storage semantics`

**Description**
The custom Table Access Method is promising, but several callbacks are currently placeholders or partial implementations. The docs should not imply mature storage semantics until the supported behavior is explicit.

This issue tracks two possible outcomes:

- complete the missing TAM surface needed for a coherent experimental storage engine, or
- narrow the advertised scope and treat the TAM strictly as a prototype path

**Acceptance criteria**

- Audit all registered TAM callbacks and mark each as implemented, partial, or unsupported
- Add tests for insert, scan, MVCC/snapshot behavior, and relation lifecycle where applicable
- Clarify WAL/durability expectations for the compact and block storage paths
- Update docs to match the actual supported semantics

**Relevant files**

- `src/tam.rs`
- `src/storage.rs`
- `README.md`

## 4. Make planner support boundaries explicit for filters, joins, and nested queries

**Title**
`planner: define and test support boundaries for filters, joins, CTEs, and nested queries`

**Description**
The planner hook is intentionally conservative, but the repo needs a sharper contract for what query shapes are accelerated versus ignored.

Areas that need explicit validation:

- arbitrary filters on non-scope columns
- join constraint propagation
- nested subqueries and CTEs
- timezone-aware slicing edge cases
- raw fallback behavior when only part of a query is eligible

**Acceptance criteria**

- Enumerate supported and unsupported query shapes
- Add regression tests for each boundary
- Ensure unsafe shapes fall back without altering query semantics
- Reduce ambiguity between README claims and actual hook scope

**Relevant files**

- `src/hooks.rs`
- `tests/pg_regress/sql`
- `docs/ARCHITECTURAL_GUIDE.md`
- `docs/QUERY_ACCELERATION.md`

## 5. Separate benchmark outputs from product claims

**Title**
`benchmarks: keep measured results in benchmark outputs and remove hard-coded performance claims from narrative docs`

**Description**
Performance numbers should live in benchmark artifacts and benchmark-focused docs, not in feature descriptions or architectural overviews.

This issue tracks a stricter documentation policy:

- narrative docs describe goals, mechanisms, and limitations
- benchmark docs contain dated measurements and methodology
- any headline numbers should be traceable to a benchmark script or result file

**Acceptance criteria**

- Remove hard-coded benchmark numbers from README-style feature descriptions
- Keep performance figures in benchmark docs/results only
- Link claims to the relevant benchmark script or result file
- Add a short documentation guideline for future updates

**Relevant files**

- `README.md`
- `docs/BENCHMARK.md`
- `docs/BENCHMARK_RESULTS.md`
- `benchmarks/`

## 6. Align compatibility claims with tested CI targets

**Title**
`build: align documented PostgreSQL compatibility with Cargo features and CI coverage`

**Description**
The repo should only claim PostgreSQL versions that are exercised by Cargo features and CI jobs in this branch.

Right now, documentation needs to stay aligned with:

- the enabled `pgrx` feature set
- CI test coverage
- release packaging targets

**Acceptance criteria**

- Document the active compatibility matrix in one place
- Keep README, Cargo features, and CI in sync
- Add new version claims only when they are tested in CI

**Relevant files**

- `Cargo.toml`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `README.md`
