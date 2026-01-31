# Agent 2 Tasks: Project Structure & Build Engineer

## Agent Role

**Primary Focus:** Clean up project structure and systematically update all references from the old crate name (claude-ai) to the new name (claude-sdk-rs) throughout the codebase.

## Key Responsibilities

- Convert project from workspace to single crate structure
- Fix examples directory structure and compilation issues
- Update all code references from claude_ai to claude_sdk_rs
- Update build scripts, Makefiles, and configuration files
- Ensure all imports and references work correctly

## Assigned Tasks

### From Original Task List

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

## Relevant Files

- `Cargo.toml` - Main project configuration needing workspace cleanup
- `examples/Cargo.toml` - Incorrect file to be removed/fixed
- `examples/*.rs` - All example files needing import updates
- `tests/**/*.rs` - All test files needing import updates
- `scripts/publish.sh` - Build script needing crate name updates
- `Makefile` - Build configuration needing updates
- `src/**/*.rs` - Source files with documentation comments to update
- `.github/workflows/*.yml` - CI configuration files (if present)

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Agent 1:** CLI and MCP modules must be building correctly (tasks 1.1.5 and 1.2.5) before updating their references

### Provides to Others (What this agent delivers)

- **To Agent 3:** Clean project structure for documentation
- **To Agent 3:** Updated examples with correct imports for documentation testing
- **To Agent 4:** Correctly configured Cargo.toml for publishing
- **To Agent 4:** Working build scripts for release process

## Handoff Points

- **After Task 2.1.5:** Notify Agent 4 that Cargo.toml structure is ready for metadata updates
- **After Task 2.2.4:** Provide Agent 3 with the list of working examples for documentation
- **After Task 3.1.5:** Notify Agent 3 that all code references are updated and examples should work
- **After Task 3.2.5:** Notify Agent 4 that build scripts are ready for publishing

## Testing Responsibilities

- Verify all examples compile with `cargo run --example <name>`
- Test build process with `cargo build --all-features`
- Validate all imports work with `cargo check --all-targets`
- Test build scripts and Makefile targets

## Notes

- Wait for Agent 1 to fix critical bugs before updating references in broken code
- Keep a log of all files changed for Agent 3's documentation
- The examples directory structure is confusing - prioritize making examples work
- Coordinate with Agent 4 on Cargo.toml changes to avoid conflicts