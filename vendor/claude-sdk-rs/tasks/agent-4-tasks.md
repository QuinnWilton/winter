# Agent 4 Tasks: Release & Compliance Engineer

## Agent Role

**Primary Focus:** Ensure legal compliance, prepare all publishing requirements, and perform final validation to make the project ready for open source release on crates.io.

## Key Responsibilities

- Create required legal files (LICENSE)
- Update Cargo.toml metadata for crates.io publishing
- Perform comprehensive validation of the entire project
- Ensure publishing readiness and security compliance
- Coordinate final release preparations

## Assigned Tasks

### From Original Task List

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

## Relevant Files

- `LICENSE` - Legal file to be created
- `Cargo.toml` - Metadata updates for publishing
- `src/lib.rs` - Main library file for documentation requirements
- `CHANGELOG.md` - Release notes and version history
- `README.md` - Referenced in Cargo.toml metadata
- `scripts/publish.sh` - Publishing script to validate
- All source files - For security and sensitive information review

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Agent 1:** Bug-free, functional codebase (tasks 1.1.5 and 1.2.6)
- **From Agent 2:** Clean Cargo.toml structure (task 2.1.5) and working build scripts (task 3.2.5)
- **From Agent 3:** Complete documentation (task 4.1.4) and verified examples (task 4.2.5)

### Provides to Others (What this agent delivers)

- **To Project:** Legal compliance and publishing readiness
- **To Team:** Final validation that project is ready for open source release
- **To Community:** Properly prepared crate for crates.io publication

## Handoff Points

- **Before Task 5.2:** Wait for Agent 2 to complete Cargo.toml structure cleanup
- **Before Task 5.3.1:** Wait for Agent 3 to confirm documentation is complete
- **After Task 5.3.6:** Final go/no-go decision for open source release

## Testing Responsibilities

- Comprehensive validation of entire project
- Security audit and sensitive information review
- Publishing dry-run validation
- Final integration testing

## Notes

- This agent has the most dependencies and should start last
- Focus on legal compliance and security - these are critical for open source
- The `cargo publish --dry-run` test is the ultimate validation
- Document any remaining issues that prevent publishing
- Coordinate with all other agents for final handoffs