# Agent Tasks: Core Systems Agent

## Agent Role

**Primary Focus:** Critical system functionality restoration and core feature implementation

## Key Responsibilities

- Fix fake streaming implementation with real streaming from CLI process
- Create comprehensive test suite for runtime crate (currently 0 tests!)
- Implement missing core features (session persistence, process cleanup)
- Clean up code quality issues across all crates

## Assigned Tasks

### From Original Task List

- [ ] 1.2 Fix Streaming Implementation - [Originally task 1.2 from main list]
  - [ ] 1.2.1 Analyze current fake streaming implementation in `claude-ai-runtime/src/client.rs`
  - [ ] 1.2.2 Implement buffered reading from stdout in `process.rs`
  - [ ] 1.2.3 Create async stream parser for StreamJson format
  - [ ] 1.2.4 Update `MessageStream` to yield messages as they arrive
  - [ ] 1.2.5 Test streaming with various response sizes and formats
  - [ ] 1.2.6 Update `streaming.rs` example to demonstrate real streaming
  - [ ] 1.2.7 Document any remaining streaming limitations

- [ ] 2.1 Create Runtime Crate Tests (Priority: CRITICAL) - [Originally task 2.1 from main list]
  - [ ] 2.1.1 Create test module structure in `claude-ai-runtime/src/`
  - [ ] 2.1.2 Write unit tests for `execute_claude()` function
  - [ ] 2.1.3 Add tests for process spawning with various configurations
  - [ ] 2.1.4 Test timeout handling and process cleanup
  - [ ] 2.1.5 Test stdin/stdout/stderr handling
  - [ ] 2.1.6 Test error scenarios (binary not found, permission denied)
  - [ ] 2.1.7 Create mock Claude CLI responses for deterministic testing
  - [ ] 2.1.8 Add integration tests for real CLI interaction

- [ ] 4.2 Implement Missing Features - [Originally task 4.2 from main list]
  - [ ] 4.2.1 Design session persistence format (JSON or SQLite)
  - [ ] 4.2.2 Implement session save/load functionality
  - [ ] 4.2.3 Add process cleanup on timeout
  - [ ] 4.2.4 Add builder validation for models and tools
  - [ ] 4.2.5 Add numeric bounds validation (timeout, max_tokens)
  - [ ] 4.2.6 Implement cancellation tokens for async tasks

- [ ] 4.3 Clean Up Code Issues - [Originally task 4.3 from main list]
  - [ ] 4.3.1 Fix all clippy warnings across all crates
  - [ ] 4.3.2 Remove unused imports and dead code
  - [ ] 4.3.3 Add missing documentation for public APIs
  - [ ] 4.3.4 Add `#![warn(missing_docs)]` to enforce documentation
  - [ ] 4.3.5 Ensure all public APIs have usage examples

## Relevant Files

### Core Implementation Files
- `claude-ai-runtime/src/process.rs` - **CRITICAL** - Main process execution, needs streaming fix and tests
- `claude-ai-runtime/src/client.rs` - Streaming implementation that needs complete rewrite
- `claude-ai/src/client.rs` - Main client that uses streaming functionality
- `claude-ai-core/src/session.rs` - Session management needing persistence
- `claude-ai-core/src/config.rs` - Configuration validation improvements

### Test Files to Create
- `claude-ai-runtime/src/process.test.rs` - **PRIORITY 1** - Unit tests for process execution
- `claude-ai-runtime/tests/integration_test.rs` - Integration tests for CLI interaction
- `claude-ai-runtime/src/lib.rs` - May need test module exports

### Examples to Update
- `claude-ai/examples/streaming.rs` - Update to demonstrate real streaming after fix

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Documentation Agent:** Need current README to understand expected API behavior
- **Independent work possible:** Most tasks can start immediately as they're fixing existing broken functionality

### Provides to Others (What this agent delivers)

- **To Testing Agent:** Working streaming implementation for streaming tests
- **To Documentation Agent:** Real streaming implementation for documentation examples
- **To Release Agent:** Clean codebase with no clippy warnings and comprehensive runtime tests

## Handoff Points

- **After Task 1.2.4:** Notify Testing Agent that real streaming is implemented for streaming tests
- **After Task 1.2.6:** Notify Documentation Agent that streaming examples are ready for documentation
- **After Task 2.1.8:** Notify Testing Agent that runtime test infrastructure is complete
- **After Task 4.3.1:** Notify Release Agent that all clippy warnings are resolved

## Testing Responsibilities

- **Critical:** Create comprehensive test suite for runtime crate (currently 0 tests)
- Unit tests for all process execution functionality
- Integration tests for real Claude CLI interaction
- Mock Claude CLI responses for deterministic testing
- Test streaming implementation thoroughly with various response sizes

## Priority Order

1. **Start with 2.1 (Runtime Tests)** - Most critical missing piece
2. **Then 1.2 (Fix Streaming)** - Core advertised feature that's broken
3. **Then 4.2 (Missing Features)** - Important but not blocking others
4. **Finally 4.3 (Code Quality)** - Polish work

## Notes

- **CRITICAL:** The runtime crate has ZERO tests - this is the highest priority for the entire project
- The streaming implementation is completely fake and needs to be rewritten from scratch
- Follow existing code conventions in the codebase
- Coordinate with Testing Agent when streaming implementation is ready for testing
- Use existing error types and patterns established in claude-ai-core
- All process spawning should use Tokio's async process APIs
- Ensure proper cleanup of child processes on timeout or cancellation