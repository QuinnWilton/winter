# claude-sdk-rs Open Source Readiness Implementation Tasks

## Metadata
- **Source PRD**: CLAUDE_SDK_RS_OPEN_SOURCE_TASKS.md
- **Generated**: 2025-06-18
- **Target Audience**: Junior Developer
- **Project**: Rebrand claude-ai to claude-sdk-rs and prepare for open source release

## Relevant Files

### Core Files to Modify
- `Cargo.toml` - Root workspace configuration that needs to become single crate manifest
- `src/lib.rs` - Main library entry point requiring feature-gated module declarations
- `README.md` - Primary documentation requiring complete rewrite for rebrand
- `CONTRIBUTING.md` - Contribution guidelines needing legal updates
- `SECURITY.md` - Security policy requiring contact information updates
- `.github/workflows/ci.yml` - CI configuration needing feature matrix updates
- `.gitignore` - Git ignore patterns requiring enhancement

### Files to Create
- `CODE_OF_CONDUCT.md` - Community code of conduct (new file)
- `ARCHITECTURE.md` - Technical architecture documentation (new file)
- `.github/workflows/release.yml` - Release automation workflow (new file)
- `examples/basic_usage.rs` - Basic SDK usage example (new file)
- `examples/streaming.rs` - Streaming response example (new file)
- `examples/error_handling.rs` - Error handling patterns (new file)
- `examples/configuration.rs` - Configuration example (new file)
- `examples/session_management.rs` - Session management example (new file)
- `examples/cli_interactive.rs` - CLI interactive mode example (new file)
- `examples/cli_analytics.rs` - CLI analytics example (new file)

### Test Files
- `tests/integration_tests.rs` - Integration tests for the consolidated crate
- `src/lib.test.rs` - Unit tests for main library functionality
- Various test files in subdirectories that need consolidation

### Notes
- Unit tests should typically be placed alongside the code files they are testing
- Use `cargo test` to run all tests, `cargo test --all-features` to test with all features enabled
- Feature flags will control which modules are compiled and tested

## Tasks

- [ ] 1.0 Rebrand Project from claude-ai to claude-sdk-rs
  - [ ] 1.1 Update project name in Cargo.toml files
    - [ ] 1.1.1 Change root Cargo.toml package name from "claude-ai" to "claude-sdk-rs"
    - [ ] 1.1.2 Update all workspace member Cargo.toml files with new naming convention
    - [ ] 1.1.3 Update internal dependency references to use new crate names
  - [ ] 1.2 Update all documentation files
    - [ ] 1.2.1 Search and replace "claude-ai" with "claude-sdk-rs" in README.md
    - [ ] 1.2.2 Update project name in CONTRIBUTING.md
    - [ ] 1.2.3 Update project name in SECURITY.md
    - [ ] 1.2.4 Update any references in CLAUDE.md
    - [ ] 1.2.5 Update project name in LICENSE file header if present
  - [ ] 1.3 Update code imports and module references
    - [ ] 1.3.1 Find all `use claude_ai` statements and update to `use claude_sdk_rs`
    - [ ] 1.3.2 Update all `extern crate claude_ai` references if any exist
    - [ ] 1.3.3 Update any string literals containing "claude-ai" in code
  - [ ] 1.4 Update repository metadata
    - [ ] 1.4.1 Update repository URL in Cargo.toml to point to new GitHub repo name
    - [ ] 1.4.2 Update homepage and documentation URLs in Cargo.toml
    - [ ] 1.4.3 Fix any hardcoded links to old repository name
  - [ ] 1.5 Update GitHub repository (if applicable)
    - [ ] 1.5.1 Rename GitHub repository from claude-ai to claude-sdk-rs
    - [ ] 1.5.2 Update any GitHub Actions that reference the old name
    - [ ] 1.5.3 Update issue templates and PR templates with new name

- [ ] 2.0 Consolidate Multi-Crate Workspace into Single Crate
  - [ ] 2.1 Create new single-crate structure
    - [ ] 2.1.1 Create `src/` directory in root if it doesn't exist
    - [ ] 2.1.2 Create new `src/lib.rs` as main entry point
    - [ ] 2.1.3 Set up basic module structure in lib.rs
  - [ ] 2.2 Merge claude-ai-core into main crate
    - [ ] 2.2.1 Copy all source files from `claude-ai-core/src/` to `src/core/`
    - [ ] 2.2.2 Update module paths in moved files to reflect new structure
    - [ ] 2.2.3 Add `pub mod core;` to lib.rs
    - [ ] 2.2.4 Move core tests to appropriate locations
  - [ ] 2.3 Merge claude-ai-runtime into main crate
    - [ ] 2.3.1 Copy all source files from `claude-ai-runtime/src/` to `src/runtime/`
    - [ ] 2.3.2 Update imports in runtime files to use crate::core instead of claude_ai_core
    - [ ] 2.3.3 Add `pub mod runtime;` to lib.rs
    - [ ] 2.3.4 Move runtime tests to appropriate locations
  - [ ] 2.4 Merge claude-ai-mcp into main crate with feature flag
    - [ ] 2.4.1 Copy all source files from `claude-ai-mcp/src/` to `src/mcp/`
    - [ ] 2.4.2 Add `#[cfg(feature = "mcp")]` attribute to mcp module
    - [ ] 2.4.3 Add `#[cfg(feature = "mcp")] pub mod mcp;` to lib.rs
    - [ ] 2.4.4 Move MCP-specific dependencies to optional dependencies in Cargo.toml
  - [ ] 2.5 Move claude-ai-interactive behind CLI feature flag
    - [ ] 2.5.1 Copy all source files from `claude-ai-interactive/src/` to `src/cli/`
    - [ ] 2.5.2 Add `#[cfg(feature = "cli")]` attributes to all CLI modules
    - [ ] 2.5.3 Add `#[cfg(feature = "cli")] pub mod cli;` to lib.rs
    - [ ] 2.5.4 Move CLI dependencies (clap, colored, etc.) to optional dependencies
    - [ ] 2.5.5 Create separate `src/bin/claude-sdk-rs.rs` for CLI binary
  - [ ] 2.6 Set up feature flags in Cargo.toml
    - [ ] 2.6.1 Define `[features]` section with `default = []`
    - [ ] 2.6.2 Add `cli` feature with required dependencies
    - [ ] 2.6.3 Add `analytics` feature for dashboard functionality
    - [ ] 2.6.4 Add `mcp` feature for Model Context Protocol
    - [ ] 2.6.5 Add `full = ["cli", "analytics", "mcp"]` feature
  - [ ] 2.7 Update main Cargo.toml
    - [ ] 2.7.1 Remove `[workspace]` section entirely
    - [ ] 2.7.2 Consolidate all dependencies from sub-crates
    - [ ] 2.7.3 Mark feature-specific dependencies as optional
    - [ ] 2.7.4 Add `[[bin]]` section for CLI binary with required-features
  - [ ] 2.8 Clean up old structure
    - [ ] 2.8.1 Remove old sub-crate directories after successful migration
    - [ ] 2.8.2 Remove workspace-specific configuration files
    - [ ] 2.8.3 Update .gitignore to reflect new structure

- [ ] 3.0 Fix All Compilation and Test Errors
  - [ ] 3.1 Fix compilation errors in main crate
    - [ ] 3.1.1 Run `cargo build` and document all errors
    - [ ] 3.1.2 Fix import paths that reference old crate names
    - [ ] 3.1.3 Update any hardcoded crate names in macros or build scripts
    - [ ] 3.1.4 Ensure all modules compile with default features
  - [ ] 3.2 Fix compilation with all features enabled
    - [ ] 3.2.1 Run `cargo build --all-features` and fix errors
    - [ ] 3.2.2 Resolve any feature flag conflicts
    - [ ] 3.2.3 Ensure optional dependencies are properly gated
  - [ ] 3.3 Fix test compilation errors
    - [ ] 3.3.1 Update test imports to use new crate structure
    - [ ] 3.3.2 Fix the 20 known compilation errors in tests
    - [ ] 3.3.3 Ensure tests compile with `cargo test --no-run`
  - [ ] 3.4 Fix test execution failures
    - [ ] 3.4.1 Run `cargo test` and fix any runtime failures
    - [ ] 3.4.2 Run `cargo test --all-features` and fix failures
    - [ ] 3.4.3 Ensure all integration tests pass
  - [ ] 3.5 Fix clippy warnings
    - [ ] 3.5.1 Run `cargo clippy` and fix all warnings
    - [ ] 3.5.2 Run `cargo clippy --all-features` and fix warnings
    - [ ] 3.5.3 Add clippy configuration if needed for project standards

- [ ] 4.0 Update Legal Compliance and Documentation
  - [ ] 4.1 Create CODE_OF_CONDUCT.md
    - [ ] 4.1.1 Use Contributor Covenant template
    - [ ] 4.1.2 Add contact information for reporting issues
    - [ ] 4.1.3 Specify enforcement guidelines
  - [ ] 4.2 Update copyright and licensing
    - [ ] 4.2.1 Search for "coldie" copyright holder and replace with proper entity
    - [ ] 4.2.2 Ensure LICENSE file is present with MIT license
    - [ ] 4.2.3 Add license headers to source files if required
  - [ ] 4.3 Update CONTRIBUTING.md
    - [ ] 4.3.1 Add section on Developer Certificate of Origin (DCO)
    - [ ] 4.3.2 Document how to sign commits
    - [ ] 4.3.3 Add pull request process and requirements
    - [ ] 4.3.4 Document coding standards and style guide
  - [ ] 4.4 Update SECURITY.md
    - [ ] 4.4.1 Add proper security contact email
    - [ ] 4.4.2 Document vulnerability reporting process
    - [ ] 4.4.3 Add security update policy
  - [ ] 4.5 Create comprehensive README.md
    - [ ] 4.5.1 Write clear project description emphasizing SDK + optional CLI
    - [ ] 4.5.2 Add installation instructions for different use cases
    - [ ] 4.5.3 Include quick start examples for SDK usage
    - [ ] 4.5.4 Document feature flags and their purposes
    - [ ] 4.5.5 Add badges for crates.io, docs.rs, CI status
  - [ ] 4.6 Create ARCHITECTURE.md
    - [ ] 4.6.1 Document high-level architecture with feature boundaries
    - [ ] 4.6.2 Explain module organization and responsibilities
    - [ ] 4.6.3 Document feature flag design decisions
    - [ ] 4.6.4 Include architecture diagrams if helpful
  - [ ] 4.7 Update .gitignore
    - [ ] 4.7.1 Add comprehensive Rust patterns
    - [ ] 4.7.2 Add IDE-specific patterns (.vscode, .idea, etc.)
    - [ ] 4.7.3 Add OS-specific patterns (.DS_Store, Thumbs.db, etc.)
    - [ ] 4.7.4 Add project-specific build artifacts

- [ ] 5.0 Prepare for crates.io Publishing
  - [ ] 5.1 Complete Cargo.toml metadata
    - [ ] 5.1.1 Write compelling description mentioning SDK and CLI capabilities
    - [ ] 5.1.2 Add keywords: ["claude", "anthropic", "ai", "sdk", "cli", "llm"]
    - [ ] 5.1.3 Add categories: ["api-bindings", "command-line-utilities"]
    - [ ] 5.1.4 Set license = "MIT"
    - [ ] 5.1.5 Set documentation = "https://docs.rs/claude-sdk-rs"
    - [ ] 5.1.6 Set homepage to GitHub repository
    - [ ] 5.1.7 Set repository to GitHub URL
    - [ ] 5.1.8 Set rust-version for MSRV (e.g., "1.70")
  - [ ] 5.2 Set up CI/CD with GitHub Actions
    - [ ] 5.2.1 Update ci.yml to test feature matrix
    - [ ] 5.2.2 Add job for `cargo test` (default features)
    - [ ] 5.2.3 Add job for `cargo test --features cli`
    - [ ] 5.2.4 Add job for `cargo test --all-features`
    - [ ] 5.2.5 Add `cargo fmt --check` to CI
    - [ ] 5.2.6 Add `cargo clippy -- -D warnings` to CI
    - [ ] 5.2.7 Remove any `continue-on-error` from test jobs
  - [ ] 5.3 Create release workflow
    - [ ] 5.3.1 Create `.github/workflows/release.yml`
    - [ ] 5.3.2 Add workflow triggers for version tags
    - [ ] 5.3.3 Add job to build CLI binaries for multiple platforms
    - [ ] 5.3.4 Add job to create GitHub release with binaries
    - [ ] 5.3.5 Add job for `cargo publish --dry-run` verification
  - [ ] 5.4 Add security scanning
    - [ ] 5.4.1 Add `cargo audit` to CI pipeline
    - [ ] 5.4.2 Configure dependabot for dependency updates
    - [ ] 5.4.3 Add RUSTSEC advisory checks
  - [ ] 5.5 Clean up dependencies
    - [ ] 5.5.1 Remove all path dependencies
    - [ ] 5.5.2 Update `dotenv` to `dotenvy` 
    - [ ] 5.5.3 Run `cargo update` to get latest compatible versions
    - [ ] 5.5.4 Review and minimize dependency tree
    - [ ] 5.5.5 Ensure no deprecated dependencies are used
  - [ ] 5.6 Create examples directory
    - [ ] 5.6.1 Implement `examples/basic_usage.rs` showing simple SDK usage
    - [ ] 5.6.2 Implement `examples/streaming.rs` demonstrating streaming responses
    - [ ] 5.6.3 Implement `examples/error_handling.rs` showing error handling patterns
    - [ ] 5.6.4 Implement `examples/configuration.rs` showing configuration options
    - [ ] 5.6.5 Implement `examples/session_management.rs` for session handling
    - [ ] 5.6.6 Implement `examples/cli_interactive.rs` with `required-features = ["cli"]`
    - [ ] 5.6.7 Implement `examples/cli_analytics.rs` with `required-features = ["analytics"]`
    - [ ] 5.6.8 Ensure all examples compile and run successfully
  - [ ] 5.7 Add comprehensive documentation
    - [ ] 5.7.1 Add rustdoc comments to all public APIs
    - [ ] 5.7.2 Include usage examples in doc comments
    - [ ] 5.7.3 Document feature flags in lib.rs module documentation
    - [ ] 5.7.4 Ensure `cargo doc` builds without warnings
    - [ ] 5.7.5 Add module-level documentation explaining architecture
  - [ ] 5.8 Final publishing checklist
    - [ ] 5.8.1 Run `cargo test --all-features` and ensure all pass
    - [ ] 5.8.2 Run `cargo clippy --all-features` with no warnings
    - [ ] 5.8.3 Run `cargo doc --all-features` with no warnings
    - [ ] 5.8.4 Test all examples: `cargo run --example basic_usage`
    - [ ] 5.8.5 Test CLI: `cargo run --features cli -- --help`
    - [ ] 5.8.6 Run `cargo publish --dry-run` successfully
    - [ ] 5.8.7 Verify no trademark issues remain
    - [ ] 5.8.8 Run security audit and fix any issues
    - [ ] 5.8.9 Ensure README clearly explains feature usage

## Implementation Notes

This task list follows the phases defined in the PRD:
- **Phase 1 (Critical)**: Tasks 1.0-3.0 - Must be completed before any public release
- **Phase 2 (High Priority)**: Task 4.0 - Required for a functional release
- **Phase 3 (Publishing)**: Task 5.0 - Preparation for crates.io publication

The consolidation approach preserves all existing functionality while simplifying the project structure. The CLI features that you worked hard on are preserved behind feature flags, allowing users to choose between a minimal SDK or the full interactive experience.

## Time Estimates
Based on the PRD:
- Phase 1: ~8 hours (Tasks 1.0-3.0)
- Phase 2: ~5 hours (Task 4.0)
- Phase 3: ~5.5 hours (Task 5.0)
- **Total: ~18.5 hours**