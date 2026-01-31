# Agent Tasks: Release & Performance Agent

## Agent Role

**Primary Focus:** Performance optimization, stub cleanup, and production release preparation

## Key Responsibilities

- Complete or remove stub implementations (macros crate)
- Optimize performance across critical code paths
- Conduct final quality checks and prepare for release
- Manage the release process and publication to crates.io

## Assigned Tasks

### From Original Task List

- [ ] 4.1 Complete or Remove Stub Implementations - [Originally task 4.1 from main list]
  - [ ] 4.1.1 Evaluate if Tool derive macro is needed
  - [ ] 4.1.2 Either implement the macro or remove from public API
  - [ ] 4.1.3 Update workspace dependencies if removing macros crate
  - [ ] 4.1.4 Document the decision in CHANGELOG

- [ ] 4.4 Optimize Performance - [Originally task 4.4 from main list]
  - [ ] 4.4.1 Profile streaming implementation
  - [ ] 4.4.2 Optimize JSON parsing for large responses
  - [ ] 4.4.3 Add connection pooling for HTTP client
  - [ ] 4.4.4 Benchmark critical paths and optimize

- [ ] 5.2 Final Quality Checks - [Originally task 5.2 from main list]
  - [ ] 5.2.1 Run full test suite across all crates
  - [ ] 5.2.2 Verify all examples compile and run correctly
  - [ ] 5.2.3 Check documentation accuracy and completeness
  - [ ] 5.2.4 Run benchmarks and document performance
  - [ ] 5.2.5 Test installation from crates.io

- [ ] 5.3 Release Process - [Originally task 5.3 from main list]
  - [ ] 5.3.1 Update version numbers in all Cargo.toml files
  - [ ] 5.3.2 Tag release version in git
  - [ ] 5.3.3 Build release artifacts
  - [ ] 5.3.4 Run publish script for crates.io
  - [ ] 5.3.5 Create GitHub release with notes
  - [ ] 5.3.6 Announce release to community

## Relevant Files

### Performance-Critical Files
- `claude-ai-runtime/src/process.rs` - Profile and optimize CLI interaction
- `claude-ai/src/client.rs` - JSON parsing optimization
- `claude-ai-runtime/src/client.rs` - Streaming performance after fix
- `claude-ai-mcp/src/lib.rs` - HTTP client optimization

### Stub Implementation Files
- `claude-ai-macros/src/lib.rs` - **DECISION NEEDED** - Complete or remove
- `claude-ai-macros/Cargo.toml` - May need removal from workspace
- `Cargo.toml` (workspace) - Dependencies cleanup if removing macros

### Release Files
- `Cargo.toml` files across all crates - Version bumping
- `CHANGELOG.md` - Release notes and version history
- `scripts/publish.sh` - Publishing automation
- `.github/workflows/release.yml` - Release automation (optional)

### Benchmark Files
- `claude-ai-interactive/benches/*.rs` - Existing benchmarks to fix
- `benches/` directories - New benchmarks to create

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Core Systems Agent:** Completed streaming implementation (Task 1.2) and clean codebase (Task 4.3)
- **From Testing Agent:** Full test suite with >80% coverage (Tasks 2.1-2.4)
- **From Documentation Agent:** Complete documentation and CI/CD pipeline (Tasks 3.1-3.3)

### Provides to Others (What this agent delivers)

- **To All Agents:** Final published release on crates.io
- **To Community:** Production-ready claude-ai SDK with performance benchmarks

## Handoff Points

- **Before Task 4.4.1:** Wait for Core Systems Agent to complete streaming implementation
- **Before Task 5.2.1:** Wait for Testing Agent to complete test suite
- **Before Task 5.2.3:** Wait for Documentation Agent to complete documentation
- **After Task 5.3.6:** Notify all agents that release is complete

## Performance Optimization Strategy

### Areas to Profile and Optimize

1. **Streaming Performance**
   - Buffer sizes for optimal throughput
   - JSON parsing efficiency for large responses
   - Memory usage during streaming

2. **Client Operations**
   - Request/response latency
   - Memory allocation patterns
   - Async task overhead

3. **Session Management**
   - Concurrent access performance
   - Memory usage for large session counts
   - Persistence operation speed

4. **HTTP Client (MCP)**
   - Connection pooling implementation
   - Request batching opportunities
   - Timeout optimization

### Benchmarking Approach

```rust
// Example benchmark categories:
- Small response processing (< 1KB)
- Medium response processing (1KB - 100KB)  
- Large response processing (> 100KB)
- Concurrent client operations
- Session management operations
- Error handling overhead
```

## Priority Order

1. **Start with 4.1 (Stub Cleanup)** - Decision needed on macros crate
2. **Wait for dependencies** - Other agents must complete their work
3. **Then 4.4 (Performance)** - Optimization after functionality is complete
4. **Then 5.2 (Quality Checks)** - Final validation
5. **Finally 5.3 (Release)** - Publication process

## Release Checklist

### Pre-Release Validation
- [ ] All tests pass across all crates
- [ ] Zero clippy warnings
- [ ] All examples compile and run
- [ ] Documentation builds successfully
- [ ] Version numbers are consistent
- [ ] CHANGELOG.md is up to date

### Performance Validation
- [ ] Benchmarks show acceptable performance
- [ ] Memory usage is reasonable
- [ ] No performance regressions from previous version
- [ ] Streaming works efficiently with large responses

### Publication Process
- [ ] Dry-run publish to verify everything works
- [ ] Publish crates in dependency order (core → runtime → main)
- [ ] Create git tags for version tracking
- [ ] Generate GitHub release with comprehensive notes
- [ ] Update documentation sites if applicable

## Stub Implementation Decision Framework

### Evaluate Tool Derive Macro:
1. **Usage Analysis:** Search codebase for actual usage
2. **Feature Completeness:** Assess implementation complexity
3. **User Value:** Determine if macro provides significant value
4. **Maintenance Cost:** Consider ongoing maintenance burden

### Options:
- **Option A:** Implement the macro if widely used and valuable
- **Option B:** Remove from public API if unused or low-value
- **Option C:** Mark as experimental/unstable if partially useful

## Notes

- **DEPENDENCY-HEAVY:** Most tasks require other agents to complete their work first
- Focus on the macros decision early as it affects workspace structure
- Use existing benchmark infrastructure where possible
- Document all performance optimizations for future reference
- Follow semantic versioning principles for release
- Coordinate release timing with other agents
- Test the actual publish process in a safe environment first
- Consider creating release automation for future versions