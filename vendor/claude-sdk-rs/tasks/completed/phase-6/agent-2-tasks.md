# Agent 2 Tasks: Build & Testing Engineer

## Agent Role

**Primary Focus:** Compilation fixes, testing, and code quality for the consolidated claude-sdk-rs crate

## Key Responsibilities

- Fix all compilation errors resulting from crate consolidation
- Ensure all tests pass with default and feature-enabled configurations
- Resolve clippy warnings and maintain code quality standards
- Validate feature flag implementations work correctly

## Assigned Tasks

### From Original Task List

- [ ] 3.0 Fix All Compilation and Test Errors - [Originally task 3.0 from main list]
  - [ ] 3.1 Fix compilation errors in main crate - [Originally task 3.1 from main list]
    - [ ] 3.1.1 Run `cargo build` and document all errors
    - [ ] 3.1.2 Fix import paths that reference old crate names
    - [ ] 3.1.3 Update any hardcoded crate names in macros or build scripts
    - [ ] 3.1.4 Ensure all modules compile with default features
  - [ ] 3.2 Fix compilation with all features enabled - [Originally task 3.2 from main list]
    - [ ] 3.2.1 Run `cargo build --all-features` and fix errors
    - [ ] 3.2.2 Resolve any feature flag conflicts
    - [ ] 3.2.3 Ensure optional dependencies are properly gated
  - [ ] 3.3 Fix test compilation errors - [Originally task 3.3 from main list]
    - [ ] 3.3.1 Update test imports to use new crate structure
    - [ ] 3.3.2 Fix the 20 known compilation errors in tests
    - [ ] 3.3.3 Ensure tests compile with `cargo test --no-run`
  - [ ] 3.4 Fix test execution failures - [Originally task 3.4 from main list]
    - [ ] 3.4.1 Run `cargo test` and fix any runtime failures
    - [ ] 3.4.2 Run `cargo test --all-features` and fix failures
    - [ ] 3.4.3 Ensure all integration tests pass
  - [ ] 3.5 Fix clippy warnings - [Originally task 3.5 from main list]
    - [ ] 3.5.1 Run `cargo clippy` and fix all warnings
    - [ ] 3.5.2 Run `cargo clippy --all-features` and fix warnings
    - [ ] 3.5.3 Add clippy configuration if needed for project standards

## Relevant Files

- `src/lib.rs` - Main entry point requiring compilation validation
- `src/core/` - Core SDK functionality needing import path fixes
- `src/runtime/` - Runtime functionality needing import updates
- `src/mcp/` - MCP functionality behind feature flag needing testing
- `src/cli/` - CLI functionality behind feature flag needing testing
- `src/bin/claude-sdk-rs.rs` - CLI binary needing compilation validation
- `tests/` - Integration tests requiring updates for new structure
- `Cargo.toml` - Dependencies and features needing validation
- Test files throughout `src/` directories needing import fixes

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Agent 1 (Consolidation):** Completed crate consolidation with basic structure in place
- **From Agent 1 (Consolidation):** Feature flags properly implemented in Cargo.toml
- **From Agent 1 (Consolidation):** All source files moved to new locations

### Provides to Others (What this agent delivers)

- **To Agent 3 (Documentation):** Confirmation that all builds and tests pass for documentation validation
- **To Agent 4 (Publishing):** Clean compilation and test suite for publishing preparation
- **To All Agents:** Compilation status reports and build validation

## Handoff Points

- **Before Task 3.1:** Wait for Agent 1 to complete basic consolidation (tasks 2.1-2.3)
- **After Task 3.1:** Report initial compilation status to Agent 1 for any structural fixes needed
- **Before Task 3.2:** Wait for Agent 1 to complete feature flag implementation (tasks 2.4-2.6)
- **After Task 3.4:** Notify Agent 4 that all tests pass and crate is ready for publishing preparation
- **After Task 3.5:** Notify all agents that code quality standards are met

## Testing Responsibilities

- **Primary Testing Owner:** All compilation and test execution
- **Feature Matrix Testing:** 
  - Default features: `cargo test`
  - CLI features: `cargo test --features cli`
  - All features: `cargo test --all-features`
- **Integration Testing:** Coordinate with other agents for cross-functional testing
- **Quality Assurance:** Ensure clippy warnings are resolved before handoff

## Specific Testing Commands

```bash
# Basic compilation testing
cargo build
cargo build --features cli
cargo build --features analytics
cargo build --features mcp
cargo build --all-features

# Test compilation
cargo test --no-run
cargo test --no-run --all-features

# Test execution
cargo test
cargo test --features cli
cargo test --all-features

# Code quality
cargo clippy
cargo clippy --all-features
cargo fmt --check
```

## Known Issues to Address

- **20 compilation errors in tests** - documented in original PRD
- **Import path updates** - from old crate names to new structure
- **Feature flag conflicts** - ensure optional dependencies are properly gated
- **Test import updates** - reflect new module structure

## Notes

- Run incremental testing as Agent 1 completes each consolidation phase
- Document all compilation errors and their fixes for future reference
- Coordinate closely with Agent 1 if structural changes are needed to fix compilation
- Maintain comprehensive test logs for Agent 4's publishing validation
- Follow existing code conventions and testing patterns
- Report any issues that require structural changes back to Agent 1 immediately