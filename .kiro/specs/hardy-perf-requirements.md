# Hardy Performance Optimization Requirements

## Executive Summary

This document specifies the functional and non-functional requirements for performance optimizations to Hardy's Bundle Processing Agent (BPA). The optimizations target hot-path allocation reduction, container efficiency improvements, and routing performance enhancements while maintaining 100% API compatibility and Hardy's reliability standards.

## Functional Requirements

### FR1: Container Optimization Requirements

#### FR1.1: HashMap Capacity Optimization
**Requirement ID**: FR1.1  
**Priority**: P0 (Critical)  
**Component**: CLA Egress Queue (`bpa/src/cla/egress_queue.rs`)

**Description**: Optimize HashMap initialization in `new_queue_set()` function to pre-allocate capacity based on known lane count.

**Acceptance Criteria**:
- HashMap created with `with_capacity(lane_count as usize + 1)`
- Zero rehashing operations during normal CLA initialization
- Identical functionality to current implementation
- No change to public API surface

**Input Constraints**:
- `lane_count`: `Option<NonZeroU32>` with maximum value `MAX_EAGER_LANE_QUEUES` (256)
- Must handle `None` case (defaults to 0 lanes)

**Output Requirements**:
- HashMap with pre-allocated capacity for exact expected size
- Same key-value mapping behavior as current implementation

#### FR1.2: SmallVec Pattern Flattening Optimization  
**Requirement ID**: FR1.2  
**Priority**: P0 (Critical)  
**Component**: Route Table (`bpa/src/routing/table.rs`)

**Description**: Optimize `flatten()` function to use `SmallVec<[EidPattern; 1]>` instead of `Vec<EidPattern>` for route pattern decomposition.

**Acceptance Criteria**:
- Use `SmallVec<[EidPattern; 1]>` as return type
- Stack allocation for single-pattern case (90%+ of usage)
- Automatic heap fallback for multi-pattern unions
- Identical pattern flattening logic and results

**Input Constraints**:
- `pattern`: `EidPattern` (single pattern or union set)
- Must handle both `EidPattern::Set` and non-set patterns

**Output Requirements**:
- Same flattened pattern sequence as current implementation
- Optimized allocation behavior for common single-pattern case

### FR2: Hot Path Allocation Requirements

#### FR2.1: Routing Table Clone Elimination
**Requirement ID**: FR2.1  
**Priority**: P1 (High)  
**Component**: Routing Information Base (`bpa/src/routing/rib.rs`)

**Description**: Eliminate expensive deep cloning of routing table in `add()` and `remove()` operations.

**Acceptance Criteria**:
- Minimize or eliminate full table cloning during route operations
- Maintain snapshot consistency for concurrent readers
- Preserve all existing route management semantics
- No impact on route lookup performance

**Input Constraints**:
- Route operations must remain atomic from reader perspective
- Snapshot updates must be thread-safe
- Must handle concurrent route modifications correctly

**Output Requirements**:
- Identical route management functionality
- Significantly reduced allocation overhead during route updates
- Maintained snapshot isolation guarantees

#### FR2.2: Early Exit Pattern Implementation
**Requirement ID**: FR2.2  
**Priority**: P1 (High)  
**Component**: Routing Information Base (`bpa/src/routing/rib.rs`)

**Description**: Add early exit optimizations to `find()` function to avoid expensive reflection setup for non-reflection cases.

**Acceptance Criteria**:
- Identify non-reflection cases before expensive reflection setup
- Skip second routing lookup when reflection not needed
- Maintain identical routing semantics and behavior
- No change to routing decision outcomes

**Input Constraints**:
- Must handle all existing routing scenarios correctly
- Bundle destination and source EID combinations
- Previous node tracking for reflection scenarios

**Output Requirements**:
- Same `DispatchAction` results as current implementation
- Reduced computational overhead for non-reflection routing
- Preserved reflection behavior when required

### FR3: String Allocation Requirements

#### FR3.1: Clone Elimination in Routing Operations
**Requirement ID**: FR3.1  
**Priority**: P2 (Medium)  
**Component**: Route Table and RIB (`bpa/src/routing/`)

**Description**: Eliminate unnecessary string clones and `Eid` clones in routing hot paths.

**Acceptance Criteria**:
- Replace `String` parameters with `&str` where possible
- Use `Cow<Eid>` or lifetime optimization for next_hop handling
- Maintain all existing string handling behavior
- No change to error message content or formatting

**Input Constraints**:
- Existing string lifetime management patterns
- API boundaries that require owned strings
- Error handling that depends on string content

**Output Requirements**:
- Identical string content in all contexts
- Reduced allocation overhead in routing operations
- Maintained error propagation and formatting

## Non-Functional Requirements

### NFR1: Performance Requirements

#### NFR1.1: Allocation Reduction Targets
**Requirement ID**: NFR1.1  
**Priority**: P0 (Critical)  

**Quantitative Targets**:
- **Hot Path Allocations**: 50-75% reduction in routing lookup allocations
- **Container Initialization**: 90%+ elimination of rehashing operations  
- **String Operations**: 60%+ reduction in string clone operations
- **Memory Pressure**: Measurable reduction in allocation frequency

**Measurement Methodology**:
- Allocation profiling using Hardy's existing benchmark suite
- Memory usage analysis during realistic bundle processing scenarios
- Comparative analysis before and after optimization implementation

#### NFR1.2: Performance Improvement Targets
**Requirement ID**: NFR1.2  
**Priority**: P0 (Critical)

**Throughput Requirements**:
- **Bundle Processing**: 10-20% improvement in bundles/second throughput
- **Route Operations**: 15-25% improvement in route add/remove performance
- **Lookup Performance**: 5-15% improvement in routing lookup latency
- **CLA Initialization**: 20-30% improvement in peer setup time

**Latency Requirements**:
- No increase in worst-case latency for any operation
- Consistent improvement across different bundle sizes and patterns
- Maintained performance under high-load scenarios

### NFR2: Compatibility Requirements

#### NFR2.1: API Compatibility  
**Requirement ID**: NFR2.1  
**Priority**: P0 (Critical)

**Public API Requirements**:
- **Zero Breaking Changes**: All existing public APIs remain unchanged
- **Signature Preservation**: Function signatures, return types, and error conditions identical
- **Behavior Consistency**: Identical functionality and semantics for all operations
- **Error Handling**: Same error conditions, types, and messages

**Integration Requirements**:
- Existing Hardy applications continue to work without modification
- No changes required to Hardy configuration or deployment
- Maintained compatibility with all Hardy storage backends and CLAs

#### NFR2.2: Semantic Compatibility
**Requirement ID**: NFR2.2  
**Priority**: P0 (Critical)

**Behavioral Requirements**:
- **Routing Decisions**: Identical routing outcomes for all bundle patterns
- **Error Propagation**: Same error conditions and recovery behavior
- **Logging Output**: Preserved debug and trace logging patterns
- **Metrics**: Maintained metrics reporting and format

**Concurrency Requirements**:
- Same thread-safety guarantees as existing implementation
- Identical behavior under concurrent access patterns
- Maintained snapshot isolation and consistency guarantees

### NFR3: Quality Requirements

#### NFR3.1: Code Quality Standards
**Requirement ID**: NFR3.1  
**Priority**: P0 (Critical)

**Hardy Compliance**:
- **Style Guide**: 100% compliance with Hardy style guide conventions
- **CI Gates**: Pass all Hardy CI requirements (build, clippy, fmt, test)
- **Documentation**: Maintain existing documentation quality and coverage
- **Architecture**: Align with Hardy's design principles and patterns

**Rust Standards**:
- **Safety**: Maintain Hardy's memory safety and 32-bit compatibility
- **Error Handling**: Use Hardy's error handling patterns (`thiserror`)
- **No-std Compatibility**: Preserve `no_std` compatibility for core components
- **Performance**: No performance regressions in non-optimized paths

#### NFR3.2: Testing Requirements
**Requirement ID**: NFR3.2  
**Priority**: P0 (Critical)

**Test Coverage**:
- **Existing Tests**: All existing tests must pass without modification
- **Regression Prevention**: No functionality regressions in any test scenario
- **Performance Tests**: Validate performance improvements with benchmarks
- **Edge Cases**: All existing edge case handling must be preserved

**Validation Requirements**:
- Comprehensive testing on all Hardy-supported platforms
- Validation under realistic bundle processing loads
- Memory usage validation and leak detection
- Concurrent access testing and safety validation

## Success Criteria

### Quantitative Success Metrics

#### Performance Metrics
- **Allocation Reduction**: Achieve 50-75% reduction in targeted hot-path allocations
- **Throughput Improvement**: Demonstrate 10-20% increase in bundle processing throughput  
- **Latency Reduction**: Show 5-15% improvement in routing lookup performance
- **Memory Efficiency**: Quantifiable reduction in memory allocation pressure

#### Quality Metrics
- **CI Compliance**: 100% pass rate for all Hardy CI gates
- **Test Coverage**: Zero test failures or behavior changes
- **Code Review**: Approval from Hardy maintainers
- **Performance Validation**: Benchmark-confirmed improvements with no regressions

### Qualitative Success Metrics

#### Integration Success
- **Natural Hardy Integration**: Changes feel native to Hardy codebase
- **Maintainability**: Code remains clear and maintainable post-optimization
- **Documentation Quality**: Clear documentation of optimization rationale
- **Community Acceptance**: Positive reception from Hardy development community

#### Long-term Success
- **Sustainability**: Optimizations provide lasting benefit to Hardy
- **Foundation**: Changes enable future performance improvements
- **Reliability**: No negative impact on Hardy's reliability or correctness
- **Adoption**: Optimizations adopted into Hardy's main development branch

## Constraints and Assumptions

### Technical Constraints
- **Hardy Version**: Optimizations target current Hardy main branch
- **Rust Version**: Must work with Hardy's current MSRV and stable toolchain
- **Platform Support**: Maintain compatibility with all Hardy-supported platforms
- **Dependencies**: No new dependencies without compelling justification

### Development Constraints
- **Timeline**: Optimizations delivered in phases over 2-3 week period
- **Resources**: Single developer implementation with community review
- **Risk Tolerance**: Conservative approach prioritizing reliability over aggressive optimization
- **Rollback Capability**: All changes must be easily revertible if issues arise

### Assumptions
- **Hardy Stability**: Hardy's core architecture remains stable during development
- **Review Process**: Hardy maintainers available for timely code review
- **Testing Environment**: Access to representative Hardy testing scenarios
- **Performance Baseline**: Current Hardy performance characteristics understood and measurable

This requirements document provides the foundation for implementing performance optimizations that deliver significant benefits while maintaining Hardy's high standards for reliability and code quality.