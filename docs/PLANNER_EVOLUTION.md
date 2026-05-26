# Evolution of the Spiral Planner: Node Pattern Catalog

## Current Limitations
- **Rigid Walking**: Current `walk_expr` and `build_time_constraints` use hardcoded logic for specific NodeTags.
- **Boolean Blindness**: Treats all `BoolExpr` as AND, leading to incorrect acceleration of OR predicates.
- **Granular SPI Calls**: Fetches metadata per-column, creating overhead in wide tables.

## The Proposal: Node Pattern Catalog
Instead of procedural "if-else" logic, we will implement a pattern-matching system inspired by compiler optimization passes.

### 1. Declarative Pattern Matching
Introduce a `Matcher` trait that can identify specific AST sub-trees:
```rust
trait NodeMatcher {
    fn matches(&self, node: *mut pg_sys::Node) -> bool;
    fn extract(&self, node: *mut pg_sys::Node) -> CapturedParams;
}
```
Patterns to implement:
- `TimeRangeBoundary`: Identifies `t >= const` or `t < const`.
- `TenantEquality`: Identifies `tenant_id = const` or `tenant_id IN (consts)`.
- `AggMergeCandidate`: Identifies aggregates that have a corresponding merge function.

### 2. Recursive Transformer
A central `TAMTransformer` that applies matches across the `Query` object:
- **Phase 1: Discovery**: Scan the `jointree` and `targetList` using the Catalog.
- **Phase 2: Neutralization**: Mark matched nodes for removal.
- **Phase 3: SQL Generation**: Build the hierarchical UNION based on captured params.
- **Phase 4: AST Grafting**: Inject the subquery and update references.

## Benefits
- **Flexibility**: Supporting a new aggregate type or predicate (like `IN`) just requires adding a new entry to the Catalog.
- **Safety**: Patterns can explicitly define their "exclusion zones" (e.g., "don't match if part of an OR tree").
- **Performance**: Batch metadata fetching can be integrated into the Discovery phase.
