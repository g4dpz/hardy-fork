# Hardy Performance Optimization Standards

## Code Quality Standards

### Hardy Style Guide Compliance
All performance optimizations must strictly adhere to the Hardy style guide as defined in [`docs/style_guides/code_style_guide.md`](../docs/style_guides/code_style_guide.md):

- **Formatting**: `rustfmt`-decided with no manual deviations
- **Use statements**: Max 3 blank-line-separated blocks (std/core/alloc, third-party, local)
- **Imports**: Collapse same-crate imports with `imports_granularity = "Crate"`
- **Visibility**: Set at definition with `pub`/`pub(crate)`, no re-export visibility changes
- **Comments**: Describe present state, no historical "moved from" narration

### Rust Idiom Requirements
- **32-bit safety**: Never `as usize` wire-derived `u64` lengths without bounds checking
- **Error handling**: Use `thiserror` enums with `#[error("...")]` per variant
- **No-std compatibility**: Core BPA components (`cbor`, `bpv7`, `bpa`) remain `no_std` + `alloc`
- **API Guidelines**: Follow [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) for all public interfaces

### Performance Optimization Principles

#### Allocation Optimization
- **Stack-first**: Use `SmallVec` for collections with predictable small sizes
- **Capacity hints**: Always provide capacity when collection size is known
- **Reuse patterns**: Prefer reusing allocations over creating new ones
- **Clone avoidance**: Use `&str` over `String`, references over owned values where possible

#### Hot Path Identification
- **Bundle processing**: Routing, dispatch, and storage operations
- **CLA operations**: Peer management, egress queues, transfer handling  
- **Route management**: Table updates, lookups, and peer resolution
- **Service delivery**: Local delivery and administrative handling

#### Conservative Changes
- **API preservation**: Zero breaking changes to public interfaces
- **Behavior maintenance**: Identical functionality and semantics
- **Error handling**: Preserve all existing error conditions and messages
- **Logging preservation**: Maintain existing debug/trace output patterns

## Performance Benchmarking Standards

### Baseline Requirements
Before implementing optimizations:
- **Establish baseline**: Run existing Hardy benchmarks (criterion-based)
- **Memory profiling**: Capture allocation patterns in hot paths
- **Bundle throughput**: Measure bundles/second in realistic scenarios
- **Route operation timing**: Measure add/remove/lookup performance

### Validation Criteria
For each optimization:
- **Improvement threshold**: Minimum 5% improvement in targeted metric
- **Regression prevention**: No degradation in any other measured metric  
- **Memory reduction**: Quantify allocation reduction in optimized paths
- **Consistency**: Performance improvement consistent across multiple runs

### Benchmarking Methodology
```rust
// Example benchmark structure for routing optimizations
fn bench_routing_lookup(c: &mut Criterion) {
    let rib = setup_realistic_rib();
    let bundles = generate_test_bundles(1000);
    
    c.bench_function("routing_lookup_baseline", |b| {
        b.iter(|| {
            for bundle in &bundles {
                black_box(rib.find(bundle));
            }
        })
    });
}
```

## Hardy-Specific Conventions

### CI Gate Compliance
All changes must pass Hardy's strict CI gates:
```bash
cargo fmt --check                                    # Zero tolerance formatting
cargo clippy --all-targets --all-features -- -D warnings  # No warnings allowed  
cargo test --locked --all-features --workspace      # All tests must pass
```

### Documentation Requirements
- **Inline comments**: Explain optimization rationale for non-obvious changes
- **Commit messages**: Include performance impact in commit message body
- **PR descriptions**: Quantify performance improvements with before/after metrics
- **Code examples**: Show allocation patterns before and after optimization

### Testing Philosophy
- **Existing tests unchanged**: Performance optimizations should not require test modifications
- **Behavior preservation**: All existing functionality must work identically
- **Edge case handling**: Optimizations must handle all existing edge cases correctly
- **Error propagation**: Error handling behavior must remain unchanged

## Optimization Categories

### Tier 1: Immediate Wins (Zero Risk)
- Container capacity hints where size is known
- Static string usage over dynamic allocation
- SmallVec for predictably small collections
- Early returns to avoid unnecessary computation

**Quality Bar**: Single-line changes with obvious correctness

### Tier 2: Quick Wins (Low Risk)  
- Clone elimination in hot paths
- Container pre-sizing optimizations
- Early exit pattern implementation
- Reference usage over owned values

**Quality Bar**: 10-20 line changes with clear benefit and low complexity

### Tier 3: Medium Wins (Medium Risk)
- Lifetime optimization for clone avoidance
- Memory reuse patterns
- Advanced SmallVec integration
- Complex allocation reduction strategies

**Quality Bar**: 25-45 line changes requiring careful review and extensive validation

## Code Review Standards

### Optimization-Specific Review Criteria
- **Performance justification**: Clear explanation of why optimization provides benefit
- **Risk assessment**: Explicit analysis of potential negative impacts
- **Alternative consideration**: Discussion of alternative approaches considered
- **Measurement**: Quantified performance improvement with benchmarks

### Hardy Integration Requirements
- **Style compliance**: Perfect adherence to Hardy formatting and conventions
- **Architecture alignment**: Changes align with Hardy's design principles
- **Dependency policy**: No new dependencies without compelling justification
- **Backward compatibility**: Zero impact on existing Hardy users

## Rollback Strategy

### Change Isolation
- **Feature branches**: Each optimization tier on separate branch
- **Incremental commits**: One logical optimization per commit
- **Revert safety**: Each commit can be safely reverted independently
- **Dependency tracking**: Clear understanding of optimization interdependencies

### Quality Gates
- **Pre-merge validation**: Comprehensive testing on feature branches
- **Performance regression detection**: Automated detection of performance degradation
- **Monitoring**: Post-merge performance monitoring for unexpected impacts
- **Rapid rollback**: Ability to quickly revert problematic optimizations

## Success Metrics

### Quantitative Targets
- **Allocation reduction**: 50-75% reduction in targeted hot paths
- **Throughput improvement**: 10-20% increase in bundles/second processing
- **Latency reduction**: Measurable improvement in routing lookup times
- **Memory efficiency**: Reduced allocation pressure and garbage collection overhead

### Qualitative Targets  
- **Code maintainability**: Optimizations improve or maintain code clarity
- **Hardy integration**: Changes feel native to Hardy's codebase
- **Community acceptance**: Optimizations accepted by Hardy maintainers
- **Long-term sustainability**: Changes that will benefit Hardy's future development

This standards document ensures all performance optimizations meet Hardy's high bar for code quality while delivering measurable performance improvements.