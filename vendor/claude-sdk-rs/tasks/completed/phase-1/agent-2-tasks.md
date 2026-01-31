# Agent Tasks: Core Systems Agent

## Agent Role

**Primary Focus:** Command discovery system and comprehensive session management infrastructure

## Key Responsibilities

- Implement the command discovery system to scan and manage .claude/commands/
- Build the complete session management system with storage and lifecycle operations
- Create abstractions and data models used by other system components
- Ensure data persistence and state management across the application

## Assigned Tasks

### From Original Task List

- [x] 2.0 Implement command discovery and listing functionality - [Originally task 2.0 from main list]
  - [x] 2.1 Build command discovery module - [Originally task 2.1 from main list]
    - [x] 2.1.1 Create CommandDiscovery struct to scan .claude/commands/ directory
    - [x] 2.1.2 Implement directory traversal with error handling for missing directories
    - [x] 2.1.3 Parse command files to extract metadata (name, description, usage)
    - [x] 2.1.4 Create Command struct to represent discovered commands
  - [x] 2.2 Implement caching mechanism - [Originally task 2.2 from main list]
    - [x] 2.2.1 Add in-memory cache for discovered commands
    - [x] 2.2.2 Implement cache invalidation when directory changes
    - [x] 2.2.3 Add filesystem watcher for automatic updates (using notify crate)
  - [x] 2.4 Write tests for command discovery - [Originally task 2.4 from main list]
    - [x] 2.4.1 Create unit tests for CommandDiscovery with mock filesystem
    - [x] 2.4.2 Test error handling for missing/malformed command files
    - [x] 2.4.3 Test filtering and caching functionality

- [x] 3.0 Build session management system - [Originally task 3.0 from main list]
  - [x] 3.1 Design session data structures - [Originally task 3.1 from main list]
    - [x] 3.1.1 Create Session struct with id, name, created_at, last_active fields
    - [x] 3.1.2 Define SessionMetadata for additional session information
    - [x] 3.1.3 Implement serialization/deserialization with serde
  - [x] 3.2 Implement session storage layer - [Originally task 3.2 from main list]
    - [x] 3.2.1 Create SessionStorage trait for abstraction
    - [x] 3.2.2 Implement JsonFileStorage using ~/.claude-ai-interactive/ directory
    - [x] 3.2.3 Handle file I/O with proper error handling and atomic writes
    - [x] 3.2.4 Create methods for load, save, delete operations
  - [x] 3.3 Build session manager - [Originally task 3.3 from main list]
    - [x] 3.3.1 Create SessionManager struct to coordinate operations
    - [x] 3.3.2 Implement create_session with unique ID generation
    - [x] 3.3.3 Add delete_session with confirmation prompt
    - [x] 3.3.4 Implement list_sessions and get_current_session
    - [x] 3.3.5 Add switch_session functionality
  - [x] 3.4 Create session CLI commands - [Originally task 3.4 from main list]
    - [x] 3.4.1 Implement SessionCommand enum with Create, Delete, List, Switch subcommands
    - [x] 3.4.2 Add proper argument parsing for each subcommand
    - [x] 3.4.3 Integrate with SessionManager for execution
    - [x] 3.4.4 Add user-friendly error messages and confirmations
  - [x] 3.5 Write session management tests - [Originally task 3.5 from main list]
    - [x] 3.5.1 Unit test SessionManager operations
    - [x] 3.5.2 Test storage persistence and recovery
    - [x] 3.5.3 Integration test full session lifecycle

## Relevant Files

- `claude-ai-interactive/src/commands/discovery.rs` - Command discovery implementation
- `claude-ai-interactive/src/commands/discovery.test.rs` - Unit tests for command discovery
- `claude-ai-interactive/src/session/manager.rs` - Session management logic
- `claude-ai-interactive/src/session/manager.test.rs` - Unit tests for session manager
- `claude-ai-interactive/src/session/storage.rs` - Session persistence layer
- `claude-ai-interactive/src/session/storage.test.rs` - Unit tests for session storage
- `claude-ai-interactive/src/cli/mod.rs` - CLI integration points

## Dependencies

### Prerequisites (What this agent needs before starting)

- **From Infrastructure Agent:** Module structure (task 1.4), error types (task 1.3.2)
- **From Infrastructure Agent:** Basic CLI framework for command integration

### Provides to Others (What this agent delivers)

- **To Infrastructure Agent:** Command discovery results for list command display
- **To Execution & Runtime Agent:** Session context and session IDs for command execution
- **To Analytics & Quality Agent:** Session data structures for history and cost tracking
- **To All Agents:** Core data models (Session, Command) used throughout the system

## Handoff Points

- **After Task 2.1:** Notify Infrastructure Agent that Command struct is ready for list command
- **After Task 3.1:** Notify Analytics & Quality Agent that Session structures are defined
- **After Task 3.3:** Notify Execution & Runtime Agent that SessionManager is ready for integration
- **Before Task 3.4.3:** Coordinate with Infrastructure Agent on CLI command integration patterns

## Testing Responsibilities

- Unit tests for all command discovery functionality
- Unit tests for session management operations
- Mock filesystem testing for command scanning
- Integration tests for session persistence and recovery
- Test coverage for error scenarios and edge cases

## Notes

- Session management is critical infrastructure - ensure robust error handling
- Command discovery should gracefully handle missing or malformed command files
- Use atomic file operations for session storage to prevent corruption
- Coordinate early with other agents on data structure design
- Consider future extensibility in trait and interface design