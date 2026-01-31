# Open Source Release Tasks for claude-sdk-rs

This document outlines all tasks required to prepare claude-sdk-rs for open source release on crates.io.

**Project Decision**: Single crate with feature flags (as configured in Cargo.toml)

## 📋 Task List

### 1. **Critical Bug Fixes** 🚨

- [ ] **1.1 Fix CLI Command Execution**
  - [ ] Fix the stubbed parallel execution in `src/cli/cli/commands.rs` (lines 519-535)
  - [ ] Resolve Arc<SessionManager> vs SessionManager type mismatch
  - [ ] Implement actual command execution instead of placeholder `Ok()`
  - [ ] Test parallel command execution functionality

- [ ] **1.2 Fix MCP Module Import Errors**
  - [ ] Correct all import paths in `src/mcp/` (240+ errors)
  - [ ] Fix circular dependencies in MCP features
  - [ ] Ensure builds with `cargo build --all-features`
  - [ ] Test MCP functionality after fixes

### 2. **Project Structure Cleanup** 🏗️

- [ ] **2.1 Clean Up Single Crate Structure**
  - [ ] Remove workspace configuration remnants from Cargo.toml
  - [ ] Delete references to non-existent workspace members
  - [ ] Ensure Cargo.toml properly defines a single crate with features

- [ ] **2.2 Fix Examples Directory**
  - [ ] Remove or fix `examples/Cargo.toml` that references non-existent paths
  - [ ] Ensure all examples compile independently
  - [ ] Test each example with `cargo run --example <name>`

### 3. **Naming and References Update** 📝

- [ ] **3.1 Update All Code References**
  - [ ] Replace all `claude_ai` imports with `claude_sdk_rs`
  - [ ] Update all `claude-ai` references to `claude-sdk-rs` in:
    - [ ] Example files (comments and code)
    - [ ] Test files
    - [ ] Documentation comments
    - [ ] Task files and internal docs

- [ ] **3.2 Update Scripts and Build Files**
  - [ ] Update `scripts/publish.sh` to use `claude-sdk-rs` crate name
  - [ ] Update Makefile to use new crate name
  - [ ] Update any other build scripts

### 4. **Documentation Completion** 📚

- [ ] **4.1 Review and Finalize Core Documentation**
  - [x] README.md created by Agent 2
  - [x] QUICK_START.md created by Agent 2
  - [x] DEV_SETUP.md created by Agent 2
  - [ ] Review and merge created documentation
  - [ ] Ensure all examples in docs are tested

- [ ] **4.2 Fix Tutorial Inconsistencies**
  - [ ] Rewrite Tutorial 06 (Session Management) to match actual API
  - [ ] Or implement the session management features described in Tutorial 06
  - [ ] Verify all code examples in tutorials compile
  - [ ] Update any remaining `claude_ai` references in tutorials

### 5. **Legal and Publishing Requirements** ⚖️

- [ ] **5.1 Add Missing Files**
  - [ ] Create LICENSE file (MIT license as specified in Cargo.toml)
  - [ ] Verify all required metadata in Cargo.toml:
    - [ ] authors
    - [ ] description
    - [ ] repository (update to correct GitHub URL)
    - [ ] keywords
    - [ ] categories
    - [ ] readme
    - [ ] documentation

- [ ] **5.2 Publishing Preparation**
  - [ ] Run `cargo publish --dry-run` and fix any issues
  - [ ] Ensure no sensitive information in codebase
  - [ ] Verify all public APIs are documented
  - [ ] Add appropriate `#![deny(missing_docs)]` if needed

### 6. **Testing and Quality Assurance** 🧪

- [ ] **6.1 Fix Existing Test Issues**
  - [ ] Update test imports from `claude_ai` to `claude_sdk_rs`
  - [ ] Fix example test compilation errors
  - [ ] Add meaningful assertions to trivial integration tests
  - [ ] Fix Config builder return type handling in tests

- [ ] **6.2 Add Missing Test Coverage**
  - [ ] Core SDK API integration tests with mocked Claude CLI
  - [ ] Streaming response parsing tests
  - [ ] Session persistence lifecycle tests
  - [ ] Tool permission validation tests
  - [ ] Concurrent client usage tests
  - [ ] Error handling edge cases

### 7. **Development Environment** 🛠️

- [ ] **7.1 Fix Developer Tools**
  - [ ] Create `.githooks/pre-commit` file
  - [ ] Add `.vscode/settings.json` for consistent development
  - [ ] Test and fix `scripts/setup-git-hooks.sh`
  - [ ] Add `.editorconfig` for consistent formatting

- [ ] **7.2 Clean Up Warnings and Lints**
  - [ ] Fix 23 missing documentation warnings
  - [ ] Address clippy warnings about excessive nesting
  - [ ] Run `cargo fmt` to fix formatting issues
  - [ ] Ensure `cargo clippy -- -D warnings` passes

### 8. **Final Validation** ✅

- [ ] **8.1 Build Validation**
  - [ ] `cargo build` passes without errors
  - [ ] `cargo build --all-features` passes without errors
  - [ ] `cargo test --all` passes
  - [ ] `cargo test --all-features` passes
  - [ ] `cargo clippy -- -D warnings` passes
  - [ ] `cargo fmt -- --check` passes

- [ ] **8.2 Documentation Validation**
  - [ ] `cargo doc --all-features --no-deps` builds without warnings
  - [ ] All examples in documentation compile and run
  - [ ] README examples work as expected
  - [ ] Tutorial code examples are valid

- [ ] **8.3 Example Validation**
  - [ ] `cargo run --example basic` works
  - [ ] `cargo run --example streaming` works
  - [ ] `cargo run --example with_tools` works
  - [ ] `cargo run --example raw_json` works
  - [ ] `cargo run --example simple` works
  - [ ] All other examples run successfully

- [ ] **8.4 Publishing Readiness**
  - [ ] `cargo publish --dry-run` succeeds
  - [ ] Version number is appropriate (1.0.0)
  - [ ] CHANGELOG.md is up to date
  - [ ] GitHub Actions CI passes (if configured)
  - [ ] Security audit with `cargo audit` passes

## Priority Order 🎯

### Phase 1: Critical Blockers (Must complete first)
- Tasks 1.1, 1.2 (Fix broken functionality)
- Tasks 2.1, 2.2 (Clean up structure)
- Task 5.1 (Add LICENSE)

### Phase 2: Essential Updates (Required for release)
- Tasks 3.1, 3.2 (Update all references)
- Tasks 4.1, 4.2 (Documentation fixes)
- Task 6.1 (Fix test issues)

### Phase 3: Quality Improvements (Should have)
- Task 6.2 (Add test coverage)
- Tasks 7.1, 7.2 (Developer experience)

### Phase 4: Final Checks (Pre-release validation)
- All of Task 8 (Final validation)

## Estimated Timeline

- **Phase 1**: 1-2 days (critical fixes)
- **Phase 2**: 1-2 days (updates and documentation)
- **Phase 3**: 1 day (quality improvements)
- **Phase 4**: 0.5 days (validation)

**Total**: 3.5-5.5 days of focused work

## Success Criteria

The project is ready for open source release when:
1. All Phase 1 and Phase 2 tasks are complete
2. `cargo publish --dry-run` succeeds
3. All examples run without errors
4. Documentation is complete and accurate
5. No references to old crate name remain

## Notes

- The project structure is a single crate with feature flags:
  - Default features: core SDK functionality
  - Optional features: `cli`, `analytics`, `mcp`, `sqlite`, `full`
- All internal module references should use relative paths
- Public API should be clean and well-documented
- Examples should demonstrate real-world usage patterns