# Task List for claude-sdk-rs Open Source Release

Generated from: `tasks/project-open-source.md`  
Date: 2025-06-19

## Relevant Files

- `src/cli/cli/commands.rs` - Contains stubbed CLI command execution that needs to be fixed
- `src/mcp/mod.rs` - Main MCP module with import errors
- `src/mcp/*` - All MCP submodules need import path corrections
- `Cargo.toml` - Needs cleanup for single crate structure and metadata updates
- `examples/Cargo.toml` - Needs to be removed or fixed
- `examples/*.rs` - All example files need import updates from claude_ai to claude_sdk_rs
- `tests/**/*.rs` - All test files need import updates
- `scripts/publish.sh` - Needs crate name updates
- `Makefile` - Needs crate name updates
- `LICENSE` - Needs to be created with MIT license
- `docs/tutorials/06-session-management.md` - Needs API consistency fixes
- `.githooks/pre-commit` - Needs to be created
- `.vscode/settings.json` - Needs to be created for consistent development

### Notes

- This is a single crate project with feature flags, not a workspace
- All references to "claude-ai" must be updated to "claude-sdk-rs"
- The project uses feature flags: `cli`, `analytics`, `mcp`, `sqlite`, `full`
- Unit tests should be alongside implementation files
- Use `cargo test --all-features` to run all tests

## Tasks

- [ ] 1.0 Fix Critical Functionality Bugs
  - [ ] 1.1 Fix CLI Command Execution
    - [ ] 1.1.1 Open `src/cli/cli/commands.rs` and locate the stubbed parallel execution (lines 519-535)
    - [ ] 1.1.2 Resolve the Arc<SessionManager> vs SessionManager type mismatch issue
    - [ ] 1.1.3 Replace the placeholder `Ok(())` with actual command execution logic
    - [ ] 1.1.4 Test parallel command execution with multiple commands
    - [ ] 1.1.5 Verify CLI commands execute properly with `cargo run --features cli`
  - [ ] 1.2 Fix MCP Module Import Errors
    - [ ] 1.2.1 Run `cargo build --all-features` and document all import errors
    - [ ] 1.2.2 Update import paths in `src/mcp/mod.rs` to use correct module structure
    - [ ] 1.2.3 Fix import paths in all MCP submodules (clients/, server/, core/, etc.)
    - [ ] 1.2.4 Resolve any circular dependency issues between MCP modules
    - [ ] 1.2.5 Verify MCP builds successfully with `cargo build --features mcp`
    - [ ] 1.2.6 Run MCP-specific tests to ensure functionality works after fixes

- [ ] 2.0 Clean Up Project Structure
  - [ ] 2.1 Convert to Single Crate Structure
    - [ ] 2.1.1 Open `Cargo.toml` and remove any `[workspace]` configuration
    - [ ] 2.1.2 Remove references to non-existent workspace members
    - [ ] 2.1.3 Ensure `[package]` section properly defines the single crate
    - [ ] 2.1.4 Verify all feature flags are properly defined in `[features]` section
    - [ ] 2.1.5 Test that `cargo build` works without workspace errors
  - [ ] 2.2 Fix Examples Directory Structure
    - [ ] 2.2.1 Delete or fix the incorrect `examples/Cargo.toml` file
    - [ ] 2.2.2 Ensure each example file has proper imports and dependencies
    - [ ] 2.2.3 Test each example individually with `cargo run --example <name>`
    - [ ] 2.2.4 Create a list of all working examples for documentation

- [ ] 3.0 Update All Project References
  - [ ] 3.1 Update Code References from claude_ai to claude_sdk_rs
    - [ ] 3.1.1 Search and replace all `use claude_ai` with `use claude_sdk_rs` in examples/
    - [ ] 3.1.2 Update all imports in test files under tests/
    - [ ] 3.1.3 Update documentation comments containing old crate name
    - [ ] 3.1.4 Fix any remaining "claude-ai" text references in code comments
    - [ ] 3.1.5 Verify all imports work with `cargo check --all-targets`
  - [ ] 3.2 Update Build Scripts and Configuration Files
    - [ ] 3.2.1 Open `scripts/publish.sh` and update all crate name references
    - [ ] 3.2.2 Update Makefile targets to use `claude-sdk-rs`
    - [ ] 3.2.3 Search for any other shell scripts that might reference old name
    - [ ] 3.2.4 Update any CI/CD configuration files if present
    - [ ] 3.2.5 Test publish script with `--dry-run` flag

- [ ] 4.0 Complete Documentation and Tutorials
  - [ ] 4.1 Finalize Core Documentation Files
    - [ ] 4.1.1 Review the generated README.md for accuracy and completeness
    - [ ] 4.1.2 Review QUICK_START.md and test all code examples
    - [ ] 4.1.3 Review DEV_SETUP.md and verify setup instructions work
    - [ ] 4.1.4 Move or merge generated docs to root directory if needed
    - [ ] 4.1.5 Ensure all documentation examples use `claude_sdk_rs`
  - [ ] 4.2 Fix Tutorial Inconsistencies
    - [ ] 4.2.1 Review `docs/tutorials/06-session-management.md` against actual API
    - [ ] 4.2.2 Either update the tutorial to match current implementation or implement missing features
    - [ ] 4.2.3 Verify all code examples in tutorials compile successfully
    - [ ] 4.2.4 Update any remaining `claude_ai` references in tutorial files
    - [ ] 4.2.5 Test tutorial code examples in a fresh project

- [ ] 5.0 Ensure Legal Compliance and Publishing Readiness
  - [ ] 5.1 Add Required Legal Files
    - [ ] 5.1.1 Create LICENSE file with MIT license text
    - [ ] 5.1.2 Verify LICENSE matches what's specified in Cargo.toml
    - [ ] 5.1.3 Add copyright header comments if required
    - [ ] 5.1.4 Check for any third-party license requirements
  - [ ] 5.2 Update Cargo.toml Metadata
    - [ ] 5.2.1 Add or update `authors` field with correct information
    - [ ] 5.2.2 Write compelling `description` for crates.io
    - [ ] 5.2.3 Update `repository` to "https://github.com/bredmond1019/claude-sdk-rust"
    - [ ] 5.2.4 Add relevant `keywords` (max 5) for discoverability
    - [ ] 5.2.5 Select appropriate `categories` for crates.io
    - [ ] 5.2.6 Ensure `readme` points to README.md
    - [ ] 5.2.7 Set `documentation` to docs.rs URL
  - [ ] 5.3 Validate Publishing Readiness
    - [ ] 5.3.1 Run `cargo publish --dry-run` and fix any errors
    - [ ] 5.3.2 Search codebase for any sensitive information (API keys, passwords)
    - [ ] 5.3.3 Add `#![warn(missing_docs)]` to lib.rs and document all public APIs
    - [ ] 5.3.4 Verify version number is set to 1.0.0
    - [ ] 5.3.5 Update CHANGELOG.md with release notes
    - [ ] 5.3.6 Run `cargo audit` to check for security vulnerabilities