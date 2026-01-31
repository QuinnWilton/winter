# Agent Tasks: Critical Infrastructure

## Agent Role

**Primary Focus:** Fix critical blocking issues and implement core infrastructure components that unblock other agents

## Key Responsibilities

- Resolve all security vulnerabilities and publish script failures
- Fix code formatting and integration test suite
- Implement session persistence and input validation
- Ensure CI/CD pipeline stability

## Assigned Tasks

### From Original Task List

- [x] 1.1 Fix Security Vulnerabilities - Originally task 1.1 from main list
  - [x] 1.1.1 Update prometheus dependency to >=0.14.0 to fix RUSTSEC-2024-0437 (protobuf vulnerability)
  - [x] 1.1.2 Replace jsonrpc-core dependency to fix RUSTSEC-2024-0421 (idna vulnerability)
  - [x] 1.1.3 Run `cargo update` to refresh Cargo.lock with new dependencies
  - [x] 1.1.4 Verify all tests pass after dependency updates
  - [x] 1.1.5 Run `cargo audit` to confirm 0 critical vulnerabilities
  - [x] 1.1.6 Update SECURITY.md with vulnerability fixes and changelog

- [x] 1.2 Fix Broken Publish Script - Originally task 1.2 from main list
  - [x] 1.2.1 Fix workspace verification logic in scripts/publish.sh
  - [x] 1.2.2 Add proper error handling for individual crate publish failures
  - [x] 1.2.3 Implement dependency waiting mechanism for crates.io availability
  - [x] 1.2.4 Test publish script with `DRY_RUN=true ./scripts/publish.sh`
  - [x] 1.2.5 Verify all crates pass `cargo publish --dry-run` individually
  - [x] 1.2.6 Document publish process in CONTRIBUTING.md

- [x] 1.3 Fix Code Formatting Violations - Originally task 1.3 from main list
  - [x] 1.3.1 Run `cargo fmt --all` to fix automatic formatting issues
  - [x] 1.3.2 Manually fix remaining style issues in benches/performance.rs
  - [x] 1.3.3 Configure git hooks to prevent future formatting violations
  - [x] 1.3.4 Verify `cargo fmt --check` reports no violations
  - [x] 1.3.5 Update CONTRIBUTING.md with formatting requirements

- [x] 1.4 Fix Integration Test Suite Failures - Originally task 1.4 from main list
  - [x] 1.4.1 Update Error enum usage from `ClaudeNotAuthenticated` to `NotAuthenticated`
  - [x] 1.4.2 Update Error enum usage from `ProcessFailed` to current variants
  - [x] 1.4.3 Fix serialization error construction in integration tests
  - [x] 1.4.4 Ensure tests verify real behavior, not just compilation
  - [x] 1.4.5 Run `cargo test --workspace` to verify 0 failures
  - [x] 1.4.6 Verify test coverage remains above 90%

- [ ] 2.4 Implement Session Persistence - Originally task 2.4 from main list
  - [ ] 2.4.1 Design storage interface for multiple backends in session.rs
  - [ ] 2.4.2 Implement file-based session storage backend
  - [ ] 2.4.3 Add SQLite database storage option for sessions
  - [ ] 2.4.4 Maintain backward compatibility with existing session management
  - [ ] 2.4.5 Add configuration options for storage backend selection
  - [ ] 2.4.6 Implement proper error handling and recovery for storage failures
  - [ ] 2.4.7 Add tests for session persistence across application restarts

- [x] 2.5 Add Comprehensive Input Validation - Originally task 2.5 from main list
  - [x] 2.5.1 Define validation rules for all user inputs in config.rs
  - [x] 2.5.2 Implement query length limits (max 100,000 characters)
  - [x] 2.5.3 Add content validation to prevent malicious input
  - [x] 2.5.4 Implement system prompt validation and length limits
  - [x] 2.5.5 Create comprehensive validation error messages
  - [x] 2.5.6 Add validation tests for edge cases and boundary conditions

## Relevant Files

- `claude-ai-mcp/Cargo.toml` - Update dependencies for security vulnerability fixes
- `scripts/publish.sh` - Fix dependency chain and error handling for publishing
- `benches/performance.rs` - Fix code formatting violations
- `claude-ai/tests/integration_test.rs` - Fix API compatibility issues in integration tests
- `claude-ai-core/src/session.rs` - Implement session persistence storage interface
- `claude-ai-core/src/config.rs` - Add comprehensive input validation
- `claude-ai-core/src/error.rs` - Enhance error handling for validation and storage
- `SECURITY.md` - Update with vulnerability fixes and security changelog
- `CONTRIBUTING.md` - Document formatting requirements and publish process
- `Cargo.lock` - Will be updated during dependency fixes

## Dependencies

### Prerequisites (What this agent needs before starting)

- **None:** This agent handles the critical blockers that must be fixed first
- **Access:** Write access to root-level configuration and security files

### Provides to Others (What this agent delivers)

- **To Service Implementation Agent:** Working CI/CD pipeline and session persistence interface
- **To Quality & Performance Agent:** Fixed test suite and security vulnerability resolution
- **To Documentation & Release Agent:** Working publish script and updated CONTRIBUTING.md

## Handoff Points

- **After Task 1.1:** Notify all agents that security vulnerabilities are resolved
- **After Task 1.2:** Notify Documentation & Release Agent that publish process is documented
- **After Task 1.3:** Notify all agents that code formatting pipeline is stable
- **After Task 1.4:** Notify Quality & Performance Agent that test suite baseline is working
- **After Task 2.4:** Notify Service Implementation Agent that session storage interface is available
- **After Task 2.5:** Notify Service Implementation Agent that input validation framework is ready

## Testing Responsibilities

- Unit tests for session persistence storage backends
- Integration tests for input validation across all APIs
- Verify security vulnerability fixes don't break existing functionality
- Ensure publish script works with dry-run before real publication

## Priority Sequence

**CRITICAL (Day 1 - Must complete first):**
1. Task 1.1 (Security) - Blocks all other security work
2. Task 1.3 (Formatting) - Unblocks CI/CD for all agents
3. Task 1.4 (Tests) - Establishes working test baseline

**HIGH (Day 2):**
4. Task 1.2 (Publish) - Needed for release readiness
5. Task 2.5 (Validation) - Blocks service implementation work

**MEDIUM (Day 3):**
6. Task 2.4 (Sessions) - Enables advanced service features

## Notes

- This agent must complete tasks 1.1, 1.3, and 1.4 before other agents can safely proceed
- Focus on getting the CI/CD pipeline stable first - this unblocks all other parallel work
- Session persistence and input validation can be worked on after critical infrastructure is stable
- Coordinate closely with other agents after completing critical blockers