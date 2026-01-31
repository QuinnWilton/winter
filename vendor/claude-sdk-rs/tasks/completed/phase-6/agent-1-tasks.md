# Agent 1 Tasks: Codebase Consolidation Specialist

## Agent Role

**Primary Focus:** Structural changes, crate consolidation, and feature flag implementation for claude-sdk-rs rebrand

## Key Responsibilities

- Execute complete rebranding from claude-ai to claude-sdk-rs
- Consolidate multi-crate workspace into single crate with feature flags
- Implement proper module structure and feature gating
- Preserve all CLI functionality behind feature flags

## Assigned Tasks

### From Original Task List

- [ ] 1.0 Rebrand Project from claude-ai to claude-sdk-rs - [Originally task 1.0 from main list]
  - [ ] 1.1 Update project name in Cargo.toml files - [Originally task 1.1 from main list]
    - [ ] 1.1.1 Change root Cargo.toml package name from "claude-ai" to "claude-sdk-rs"
    - [ ] 1.1.2 Update all workspace member Cargo.toml files with new naming convention
    - [ ] 1.1.3 Update internal dependency references to use new crate names
  - [ ] 1.2 Update all documentation files - [Originally task 1.2 from main list]
    - [ ] 1.2.1 Search and replace "claude-ai" with "claude-sdk-rs" in README.md
    - [ ] 1.2.2 Update project name in CONTRIBUTING.md
    - [ ] 1.2.3 Update project name in SECURITY.md
    - [ ] 1.2.4 Update any references in CLAUDE.md
    - [ ] 1.2.5 Update project name in LICENSE file header if present
  - [ ] 1.3 Update code imports and module references - [Originally task 1.3 from main list]
    - [ ] 1.3.1 Find all `use claude_ai` statements and update to `use claude_sdk_rs`
    - [ ] 1.3.2 Update all `extern crate claude_ai` references if any exist
    - [ ] 1.3.3 Update any string literals containing "claude-ai" in code
  - [ ] 1.4 Update repository metadata - [Originally task 1.4 from main list]
    - [ ] 1.4.1 Update repository URL in Cargo.toml to point to new GitHub repo name
    - [ ] 1.4.2 Update homepage and documentation URLs in Cargo.toml
    - [ ] 1.4.3 Fix any hardcoded links to old repository name
  - [ ] 1.5 Update GitHub repository (if applicable) - [Originally task 1.5 from main list]
    - [ ] 1.5.1 Rename GitHub repository from claude-ai to claude-sdk-rs
    - [ ] 1.5.2 Update any GitHub Actions that reference the old name
    - [ ] 1.5.3 Update issue templates and PR templates with new name

- [ ] 2.0 Consolidate Multi-Crate Workspace into Single Crate - [Originally task 2.0 from main list]
  - [ ] 2.1 Create new single-crate structure - [Originally task 2.1 from main list]
    - [ ] 2.1.1 Create `src/` directory in root if it doesn't exist
    - [ ] 2.1.2 Create new `src/lib.rs` as main entry point
    - [ ] 2.1.3 Set up basic module structure in lib.rs
  - [ ] 2.2 Merge claude-ai-core into main crate - [Originally task 2.2 from main list]
    - [ ] 2.2.1 Copy all source files from `claude-ai-core/src/` to `src/core/`
    - [ ] 2.2.2 Update module paths in moved files to reflect new structure
    - [ ] 2.2.3 Add `pub mod core;` to lib.rs
    - [ ] 2.2.4 Move core tests to appropriate locations
  - [ ] 2.3 Merge claude-ai-runtime into main crate - [Originally task 2.3 from main list]
    - [ ] 2.3.1 Copy all source files from `claude-ai-runtime/src/` to `src/runtime/`
    - [ ] 2.3.2 Update imports in runtime files to use crate::core instead of claude_ai_core
    - [ ] 2.3.3 Add `pub mod runtime;` to lib.rs
    - [ ] 2.3.4 Move runtime tests to appropriate locations
  - [ ] 2.4 Merge claude-ai-mcp into main crate with feature flag - [Originally task 2.4 from main list]
    - [ ] 2.4.1 Copy all source files from `claude-ai-mcp/src/` to `src/mcp/`
    - [ ] 2.4.2 Add `#[cfg(feature = "mcp")]` attribute to mcp module
    - [ ] 2.4.3 Add `#[cfg(feature = "mcp")] pub mod mcp;` to lib.rs
    - [ ] 2.4.4 Move MCP-specific dependencies to optional dependencies in Cargo.toml
  - [ ] 2.5 Move claude-ai-interactive behind CLI feature flag - [Originally task 2.5 from main list]
    - [ ] 2.5.1 Copy all source files from `claude-ai-interactive/src/` to `src/cli/`
    - [ ] 2.5.2 Add `#[cfg(feature = "cli")]` attributes to all CLI modules
    - [ ] 2.5.3 Add `#[cfg(feature = "cli")] pub mod cli;` to lib.rs
    - [ ] 2.5.4 Move CLI dependencies (clap, colored, etc.) to optional dependencies
    - [ ] 2.5.5 Create separate `src/bin/claude-sdk-rs.rs` for CLI binary
  - [ ] 2.6 Set up feature flags in Cargo.toml - [Originally task 2.6 from main list]
    - [ ] 2.6.1 Define `[features]` section with `default = []`
    - [ ] 2.6.2 Add `cli` feature with required dependencies
    - [ ] 2.6.3 Add `analytics` feature for dashboard functionality
    - [ ] 2.6.4 Add `mcp` feature for Model Context Protocol
    - [ ] 2.6.5 Add `full = ["cli", "analytics", "mcp"]` feature
  - [ ] 2.7 Update main Cargo.toml - [Originally task 2.7 from main list]
    - [ ] 2.7.1 Remove `[workspace]` section entirely
    - [ ] 2.7.2 Consolidate all dependencies from sub-crates
    - [ ] 2.7.3 Mark feature-specific dependencies as optional
    - [ ] 2.7.4 Add `[[bin]]` section for CLI binary with required-features
  - [ ] 2.8 Clean up old structure - [Originally task 2.8 from main list]
    - [ ] 2.8.1 Remove old sub-crate directories after successful migration
    - [ ] 2.8.2 Remove workspace-specific configuration files
    - [ ] 2.8.3 Update .gitignore to reflect new structure

## Relevant Files

- `Cargo.toml` - Root workspace configuration to transform into single crate manifest
- `claude-ai-core/src/` - Core SDK functionality to merge into `src/core/`
- `claude-ai-runtime/src/` - Runtime functionality to merge into `src/runtime/`
- `claude-ai-mcp/src/` - MCP functionality to merge into `src/mcp/` with feature flag
- `claude-ai-interactive/src/` - CLI functionality to merge into `src/cli/` with feature flag
- `src/lib.rs` - New main entry point requiring feature-gated module declarations
- `src/bin/claude-sdk-rs.rs` - CLI binary entry point to create
- `README.md` - Primary documentation requiring rebranding
- `CONTRIBUTING.md` - Contribution guidelines requiring name updates
- `SECURITY.md` - Security policy requiring name updates
- `CLAUDE.md` - Project documentation requiring updates
- `LICENSE` - License file potentially requiring header updates
- `.gitignore` - Git patterns requiring updates for new structure

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Project Setup:** Access to current codebase structure
- **From Planning:** Confirmation of new feature flag architecture design

### Provides to Others (What this agent delivers)

- **To Agent 2 (Build & Testing):** Consolidated single-crate structure ready for compilation testing
- **To Agent 3 (Documentation):** Updated project name and basic structure for documentation work
- **To Agent 4 (Publishing):** Proper Cargo.toml structure and feature flags for publishing setup

## Handoff Points

- **After Task 1.0:** Notify all agents that rebranding is complete - no more "claude-ai" references
- **After Task 2.1-2.3:** Notify Agent 2 that basic consolidation is ready for initial build testing
- **After Task 2.4-2.5:** Notify Agent 2 that feature flags are implemented and ready for full testing
- **After Task 2.7:** Notify Agent 4 that Cargo.toml structure is ready for publishing metadata
- **After Task 2.8:** Notify all agents that structural consolidation is complete

## Testing Responsibilities

- Basic compilation testing: `cargo build` should work after each major merge step
- Feature flag testing: `cargo build --features cli`, `cargo build --all-features`
- Module structure validation: ensure all imports resolve correctly
- Coordinate with Agent 2 for comprehensive testing after consolidation

## Notes

- Follow existing code conventions in the codebase
- Preserve ALL CLI functionality - this was explicitly requested by the user
- Use feature flags to maintain backward compatibility
- Test compilation after each major structural change
- Create backup of original structure before starting major moves
- Coordinate with Agent 2 at key milestones to ensure builds remain functional