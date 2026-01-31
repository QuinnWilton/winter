# Agent 2 Tasks: Testing & Coverage Agent

## Agent Role

**Primary Focus:** Complete test coverage, verify test metrics accuracy, and document testing strategies

## Key Responsibilities

- Verify and correct test count claims
- Add missing test coverage to reach targets
- Document test organization and best practices
- Set up test coverage reporting

## Assigned Tasks

### From Original Task List

- [x] 2.0 Complete Test Coverage and Documentation
  - [x] 2.1 Verify and Correct Test Metrics
    - [x] 2.1.1 Count actual tests in all test files using grep/script
    - [x] 2.1.2 Update documentation with accurate test counts
    - [x] 2.1.3 Add test counting command to Makefile
    - [x] 2.1.4 Create test inventory markdown file
  - [x] 2.2 Add Missing Test Coverage
    - [x] 2.2.1 Add 5 missing tests to reach 89 total in core
    - [x] 2.2.2 Add streaming timeout edge case tests
    - [x] 2.2.3 Add concurrent streaming request tests
    - [x] 2.2.4 Add malformed CLI output handling tests
    - [x] 2.2.5 Add property-based tests for Config validation
    - [x] 2.2.6 Add integration tests for error recovery
    - [x] 2.2.7 Set up test coverage reporting with tarpaulin
  - [x] 2.3 Improve Test Documentation
    - [x] 2.3.1 Document test organization in docs/TESTING.md
    - [x] 2.3.2 Create test writing guidelines with examples
    - [x] 2.3.3 Document mock vs real CLI testing strategy
    - [x] 2.3.4 Add test examples to CONTRIBUTING.md

## Relevant Files

- `claude-ai-core/src/config_test.rs` - Add missing tests (currently 36, need more)
- `claude-ai-core/src/session_test.rs` - Verify test count (currently 26)
- `claude-ai-runtime/tests/process_tests.rs` - Add timeout and edge case tests
- `claude-ai-runtime/tests/integration_tests.rs` - Add concurrent request tests
- `claude-ai/src/client_test.rs` - Verify count and add scenarios
- `Makefile` - Add test counting command
- `docs/TESTING.md` - Document test organization
- `CONTRIBUTING.md` - Add test examples
- `docs/TEST_INVENTORY.md` - New file to create
- `.github/workflows/ci.yml` - Configure tarpaulin for coverage

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Code Quality Agent:** Clean test files without clippy warnings (after task 1.1)

### Provides to Others (What this agent delivers)

- **To Release Agent:** Accurate test metrics for documentation
- **To Performance Agent:** Test coverage data for CI/CD configuration
- **To All Agents:** Test writing guidelines and examples

## Handoff Points

- **After Task 2.1.4:** Share test inventory with Release Agent for documentation
- **After Task 2.2.7:** Notify Performance Agent that coverage reporting is ready
- **After Task 2.3.4:** Confirm test guidelines are available for all agents

## Testing Responsibilities

- Primary owner of all test strategy and implementation
- Ensure all new tests follow established patterns
- Maintain >80% code coverage target
- Verify test reliability (no flaky tests)

## Notes

- Current test counts: Core has 84 tests (claimed 89), so 5 are missing
- Focus on edge cases and error scenarios for new tests
- Property-based tests should use `proptest` crate (already in dependencies)
- Coordinate with Performance Agent on streaming timeout tests
- Mock CLI testing is critical for deterministic results