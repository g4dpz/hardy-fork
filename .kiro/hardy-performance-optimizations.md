# Hardy Performance Optimizations Spec

## Overview

This spec outlines low-effort, high-value performance optimizations for Hardy's Bundle Processing Agent (BPA) targeting allocation reduction, container optimization, and hot-path efficiency improvements. All optimizations maintain API compatibility and focus on the most frequently executed code paths.

## Goals

- **Reduce heap allocations** in bundle processing hot paths
- **Optimize container initialization** with proper capacity hints
- **Eliminate unnecessary string clones** in routing operations
- **Add early exit optimizations** to avoid expensive operations
- **Maintain 100% API compatibility** with existing Hardy interfaces

## Performance Targets

- **Allocation Reduction**: 50-75% reduction in hot-path allocations
- **Routing Performance**: 10-20% improvement in bundle/second throughput
- **Memory Efficiency**: Reduced allocation pressure and GC overhead
- **Zero Breaking Changes**: All optimizations are internal implementation details

## Requirements

### R1: Immediate Wins (Tier 1 - Highest Priority)

#### R1.1: CLA Egress Queue HashMap Capacity Optimization
- **File**: `bpa/src/cla/egress_queue.rs:54`
- **Issue**: HashMap created without capacity hint despite knowing exact size needed
- **Solution**: Use `HashMap::with_capacity(lane_count as usize + 1)`
- **Impact**: Eliminates rehashing during CLA initialization
- **Effort**: 1 line change

#### R1.2: Route Pattern Flattening SmallVec Optimization  
- **File**: `bpa/src/routing/table.rs:356` (flatten function)
- **Issue**: Returns `Vec<EidPattern>` for every union route, but most patterns are single items
- **Solution**: Use `SmallVec<[EidPattern; 1]>` to avoid heap allocation for common case
- **Impact**: Eliminates allocation for ~90% of route patterns
- **Effort**: 2 line change

### R2: Quick Wins (Tier 2 - High Priority)

#### R2.1: Routing Table Deep Clone Elimination
- **File**: `bpa/src/routing/rib.rs:243` (add function)
- **Issue**: Expensive deep clone of entire routing table on every add/remove operation
- **Solution**: Optimize snapshot management to avoid unnecessary cloning
- **Impact**: Significant improvement in route management operations
- **Effort**: 15-20 lines

#### R2.2: Early Exit Optimization in Route Lookup
- **File**: `bpa/src/routing/rib.rs:138-186` (find function)
- **Issue**: Reflection case setup executed even for non-reflection lookups
- **Solution**: Add early returns for common non-reflection cases
- **Impact**: Avoids expensive reflection branch setup for majority of lookups
- **Effort**: 10-15 lines

#### R2.3: Container Pre-sizing in Route Table Construction
- **Files**: `bpa/src/routing/table.rs:112,123`
- **Issue**: BTreeMap containers created without capacity hints
- **Solution**: Use appropriate initial capacity for admin_endpoints and routes
- **Impact**: Reduces rehashing during route table initialization
- **Effort**: 5-10 lines

### R3: Medium Wins (Tier 3 - Medium Priority)

#### R3.1: Next Hop Clone Elimination in Routing Hot Path
- **File**: `bpa/src/routing/rib.rs:162,180` (find function)
- **Issue**: Multiple `Eid` clones in bundle routing hot path
- **Solution**: Use `Cow<Eid>` or better lifetime management to avoid clones
- **Impact**: Reduces allocation pressure in bundle dispatch
- **Effort**: 25-40 lines

#### R3.2: SmallVec Reuse Optimization in Route Lookup
- **File**: `bpa/src/routing/table.rs:250` (find_recurse)
- **Issue**: New SmallVec allocated on every bundle routing lookup
- **Solution**: Reuse SmallVec or pre-allocate with appropriate capacity
- **Impact**: Eliminates allocation in critical bundle processing path
- **Effort**: 30-45 lines

#### R3.3: CLA Forward Early Exit Optimization
- **File**: `bpa/src/cla/peers.rs:108-118` (forward function)
- **Issue**: Flow label classification performed even when CLA upgrade unavailable
- **Solution**: Check CLA availability before expensive classification
- **Impact**: Avoids unnecessary work in failed forward attempts
- **Effort**: 5-10 lines

## Design Approach

### Phase 1: Foundation Optimizations (Tier 1)
- Implement immediate wins with minimal risk
- Establish performance baseline with benchmarks
- Validate CI gates pass for all changes

### Phase 2: Hot Path Optimizations (Tier 2) 
- Focus on routing and dispatch critical paths
- Implement early exit patterns
- Optimize container initialization

### Phase 3: Advanced Optimizations (Tier 3)
- Memory management improvements
- Lifetime optimization for clone elimination
- SmallVec reuse patterns

## Implementation Strategy

### Development Workflow
1. **Create feature branch** for each optimization tier
2. **Implement optimizations** in dependency order
3. **Run Hardy CI gates** (build, clippy, fmt, test)
4. **Performance validation** with existing benchmarks
5. **Create PR** with detailed performance analysis

### Code Quality Requirements
- **Follow Hardy style guide** conventions exactly
- **Maintain existing error handling** patterns
- **Preserve all existing functionality** and behavior
- **Add inline comments** explaining optimization rationale
- **Include performance impact** in commit messages

### Testing Strategy
- **All existing tests must pass** without modification
- **No new test coverage required** (internal optimizations)
- **Performance regression prevention** via benchmark validation
- **Memory usage validation** where applicable

## Success Criteria

### Performance Metrics
- **Bundle throughput improvement**: 10-20% increase in bundles/second
- **Allocation reduction**: 50-75% fewer allocations in hot paths
- **Route management improvement**: Faster route add/remove operations
- **Memory pressure reduction**: Lower allocation frequency

### Quality Metrics
- **Zero API breaking changes**: All existing code continues to work
- **CI gate compliance**: build ✓, clippy ✓, fmt ✓, test ✓
- **Code review approval**: Performance improvements validated
- **Backward compatibility**: No behavior changes for existing functionality

### Delivery Timeline
- **Phase 1 (Immediate)**: 1-2 days - Tier 1 optimizations
- **Phase 2 (Quick)**: 3-5 days - Tier 2 optimizations  
- **Phase 3 (Medium)**: 1-2 weeks - Tier 3 optimizations

## Risk Assessment

### Low Risk Optimizations
- Container capacity hints (HashMap, SmallVec)
- Early exit patterns
- Static string usage

### Medium Risk Optimizations  
- Lifetime management changes
- Clone elimination patterns
- Memory reuse strategies

### Mitigation Strategies
- **Incremental implementation** in small, reviewable chunks
- **Comprehensive testing** at each phase
- **Performance validation** before and after
- **Rollback plan** for any optimization causing issues

## Out of Scope

- **Breaking API changes**: All optimizations must be internal
- **New dependencies**: Use existing Hardy dependencies only
- **Behavioral changes**: Maintain existing Hardy semantics exactly
- **Cross-crate optimizations**: Focus on BPA crate primarily

This spec provides a structured approach to implementing performance optimizations that will significantly improve Hardy's bundle processing efficiency while maintaining the project's high standards for correctness and reliability.