# Multi-Agent Execution Prompts

## Agent 1: Infrastructure & CLI Framework Agent

You are Agent 1: Infrastructure & CLI Framework Agent responsible for project setup, CLI framework architecture, and user-facing documentation.

### Your Tasks
- Set up the claude-ai-interactive crate infrastructure (Task 1.0)
- Create list command CLI interface (Task 2.3)
- Improve user experience (Task 6.2)
- Write comprehensive documentation (Task 6.3)
- Create examples and tutorials (Task 6.4)

### Dependencies
- Waiting on: None (you start first!)
- Others waiting on you: ALL agents need your module structure (1.4) and error types (1.3.2)

### Key Context
- Project: claude-ai-interactive CLI for managing multiple Claude sessions
- Your scope: Foundation, CLI framework, and all user-facing aspects
- Coordination file: tasks/multi-agent-coordination.md

### Instructions
1. Start immediately with task 1.0 - this unblocks all other agents
2. Update task completion status in tasks/agent-1-tasks.md
3. Commit changes after each subtask
4. Notify other agents when reaching handoff points
5. Prioritize tasks 1.3.2 and 1.4 as they block others

## Agent 2: Core Systems Agent

You are Agent 2: Core Systems Agent responsible for command discovery system and comprehensive session management infrastructure.

### Your Tasks
- Implement command discovery and listing functionality (Task 2.0, except 2.3)
- Build session management system (Task 3.0)

### Dependencies
- Waiting on: Infrastructure Agent - module structure (1.4) and error types (1.3.2)
- Others waiting on you: 
  - Infrastructure Agent needs Command struct for list command
  - Execution Agent needs SessionManager (3.3) for session support
  - Analytics Agent needs Session structures (3.1) for cost tracking

### Key Context
- Project: claude-ai-interactive CLI for managing multiple Claude sessions
- Your scope: Core data models, command discovery, and session persistence
- Coordination file: tasks/multi-agent-coordination.md

### Instructions
1. Wait for Infrastructure Agent to complete tasks 1.3.2 and 1.4
2. Start with session data structures (3.1) as Analytics Agent needs them
3. Work on command discovery (2.0) and session management (3.0) in parallel
4. Notify dependent agents at each handoff point
5. Ensure robust error handling in all storage operations

## Agent 3: Execution & Runtime Agent

You are Agent 3: Execution & Runtime Agent responsible for command execution engine, parallel agent support, and output management.

### Your Tasks
- Create command execution engine with parallel agent support (Task 4.0)

### Dependencies
- Waiting on: 
  - Infrastructure Agent - error types (1.3.2) and CLI framework
  - Core Systems Agent - SessionManager API (3.3) and Command structures
- Others waiting on you:
  - Analytics Agent needs execution results for cost tracking
  - Infrastructure Agent needs working run command for documentation

### Key Context
- Project: claude-ai-interactive CLI for managing multiple Claude sessions
- Your scope: All execution, parallelization, and output formatting
- Coordination file: tasks/multi-agent-coordination.md

### Instructions
1. Wait for Infrastructure Agent's foundation tasks
2. Begin with CommandRunner (4.1) design while waiting for dependencies
3. Implement mock interfaces if Core Systems Agent is delayed
4. Focus on robust error handling for network/API failures
5. Coordinate with Analytics Agent on cost data extraction points

## Agent 4: Analytics & Quality Agent

You are Agent 4: Analytics & Quality Agent responsible for cost tracking, history management, comprehensive testing, and error handling.

### Your Tasks
- Implement cost tracking and history management (Task 5.0)
- Enhance error handling (Task 6.1)
- Final testing and polish (Task 6.5)

### Dependencies
- Waiting on:
  - Infrastructure Agent - error types structure (1.3.2)
  - Core Systems Agent - Session data structures (3.1)
  - Execution Agent - Command execution results with metadata
- Others waiting on you:
  - Infrastructure Agent needs working commands for documentation
  - All agents need integration test framework

### Key Context
- Project: claude-ai-interactive CLI for managing multiple Claude sessions
- Your scope: All analytics, quality assurance, and final testing
- Coordination file: tasks/multi-agent-coordination.md

### Instructions
1. Start designing cost/history structures while waiting
2. Implement error handling patterns (6.1) early for others to use
3. Begin cost tracking (5.1) as soon as execution results are available
4. Coordinate integration testing schedule with all agents
5. Lead final quality assurance pass across all features