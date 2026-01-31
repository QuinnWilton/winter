# 🔧 Development Setup Guide

This guide covers everything you need to contribute to `claude-sdk-rs` or build it from source.

## 📋 Prerequisites

### Required Tools

1. **Rust** (1.70 or later)
   ```bash
   # Install Rust
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   
   # Verify installation
   rustc --version
   cargo --version
   ```

2. **Claude Code CLI**
   ```bash
   # Install from https://claude.ai/code
   # Then authenticate:
   claude auth
   ```

3. **Git**
   ```bash
   git --version
   ```

### Recommended Tools

- **rust-analyzer** - IDE support
- **cargo-watch** - Auto-rebuild on changes
- **cargo-expand** - Macro expansion
- **cargo-audit** - Security audits

```bash
cargo install cargo-watch cargo-expand cargo-audit
```

## 🚀 Getting Started

### 1. Clone the Repository

```bash
git clone https://github.com/bredmond1019/claude-sdk-rust.git
cd claude-sdk-rust
```

### 2. Build the Project

```bash
# Build the main crate (core functionality only)
cargo build

# Build with specific features
cargo build --features cli
cargo build --features sqlite

# Build with all features (includes CLI binary)
cargo build --all-features

# Build in release mode
cargo build --release
```

⚠️ **Known Issues**: 
- MCP and SQLite features currently have compilation errors
- Use `cargo build --features cli` for CLI functionality
- Core SDK functionality (without features) works correctly

### 3. Run Tests

```bash
# Run core library tests (recommended for development)
cargo test --lib

# Run all tests (may take longer, some tests require timeouts)
cargo test --timeout 300

# Run tests with basic features
cargo test --features cli

# Run with verbose output
cargo test --lib -- --nocapture

# Run integration tests only
cargo test --test '*'

# Run documentation tests
cargo test --doc
```

⚠️ **Testing Notes**: 
- Some tests have long runtimes and may timeout with default settings
- Use `--lib` flag for faster unit test runs during development
- Integration tests require Claude CLI to be installed and authenticated

## 🏗️ Project Architecture

The project is organized as a single crate with modular structure:

```
claude-sdk-rs/
├── Cargo.toml              # Package configuration
├── src/                    # Main SDK crate (claude-sdk-rs)
│   ├── lib.rs             # Public API
│   ├── bin/               # CLI binary (feature-gated)
│   ├── core/              # Core types and config
│   ├── runtime/           # Process execution
│   ├── mcp/               # MCP protocol (feature-gated)
│   └── cli/               # CLI interface (feature-gated)
├── examples/               # Usage examples
├── tests/                  # Integration tests
├── benches/               # Performance benchmarks
├── scripts/               # Development scripts
└── docs/                   # Documentation
```

### Key Modules

1. **Core** (`src/core/`)
   - Configuration and builders
   - Error types
   - Message types
   - Session management

2. **Runtime** (`src/runtime/`)
   - Claude CLI process execution
   - Response streaming
   - Backpressure handling

3. **MCP** (`src/mcp/`) - *Optional*
   - Model Context Protocol
   - Tool integration
   - External services

4. **CLI** (`src/cli/`) - *Optional*
   - Interactive terminal interface
   - Command processing
   - Analytics dashboard

## 🛠️ Development Workflow

### 1. Feature Development

```bash
# Create a feature branch
git checkout -b feature/your-feature-name

# Make changes and test
cargo test
cargo clippy
cargo fmt

# Run examples to verify
cargo run --example basic_usage
```

### 2. Code Quality

```bash
# Format code
cargo fmt

# Run linter (expect warnings due to development state)
cargo clippy

# Run linter with stricter settings (will show many warnings)
cargo clippy -- -D warnings

# Check compilation without features (fastest)
cargo check

# Check with features
cargo check --features cli

# Security audit (install if needed)
cargo install cargo-audit
cargo audit

# Use Makefile for comprehensive checks
make dev  # Runs fmt, lint, and test
```

⚠️ **Code Quality Notes**: 
- The codebase currently has clippy warnings that need addressing
- Use `cargo clippy` (without `-D warnings`) during development
- Run `make dev` for the full development workflow

### 3. Testing

#### Unit Tests
```rust
// Add tests in the same file as the code
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_builder() {
        let config = Config::builder()
            .model("claude-opus-4")
            .build()
            .unwrap();
        assert_eq!(config.model, "claude-opus-4");
    }
}
```

#### Integration Tests
```rust
// In tests/integration_test.rs
use claude_sdk_rs::{Client, Config};

#[tokio::test]
async fn test_basic_query() {
    let client = Client::new(Config::default());
    let result = client.query("Hello").send().await;
    assert!(result.is_ok());
}
```

### 4. Documentation

```bash
# Build documentation
cargo doc --all-features --no-deps

# Build and open docs
cargo doc --all-features --no-deps --open

# Test documentation examples
cargo test --doc
```

## 📦 Building Features

### Feature Flags

The SDK uses Cargo features for optional functionality:

```toml
[features]
default = []                         # Core SDK only
cli = ["clap", "colored", ...]       # CLI binary and interface
mcp = ["tokio-tungstenite", ...]     # MCP protocol (⚠️ compilation issues)
sqlite = ["sqlx"]                    # SQLite storage (⚠️ compilation issues)
analytics = ["cli"]                  # Analytics dashboard (requires CLI)
full = ["cli", "analytics", "mcp", "sqlite"]  # All features
```

**Current Feature Status**:
- ✅ **default**: Core SDK functionality - working
- ✅ **cli**: Command-line interface - working  
- ✅ **analytics**: Analytics dashboard - working (requires cli)
- ⚠️ **mcp**: Model Context Protocol - compilation errors
- ⚠️ **sqlite**: SQLite session storage - compilation errors
- ⚠️ **full**: All features - compilation errors due to mcp/sqlite

### Building with Features

```bash
# Build with specific features
cargo build --features cli
cargo build --features mcp,sqlite
cargo build --all-features

# Test with features
cargo test --features cli
cargo test --all-features
```

## 🧪 Running Examples

The `examples/` directory contains various usage examples:

```bash
# List all examples
ls examples/*.rs

# Run core SDK examples (these work reliably)
cargo run --example basic_usage
cargo run --example streaming
cargo run --example session_management
cargo run --example error_handling
cargo run --example configuration

# Run CLI examples (requires cli feature)
cargo run --example cli_interactive --features cli
cargo run --example cli_analytics --features analytics

# Check which examples are available
find examples -name "*.rs" -not -path "*/tests/*" | sort
```

**Verified Working Examples**:
- ✅ `basic_usage` - Core SDK demonstration
- ✅ `streaming` - Streaming responses
- ✅ `session_management` - Session handling
- ✅ CLI examples work with `--features cli`

## 📊 Performance Testing

### Benchmarks

```bash
# Run benchmarks
cargo bench

# Run specific benchmark
cargo bench performance

# Compare benchmarks
cargo bench -- --save-baseline main
# Make changes...
cargo bench -- --baseline main
```

### Profiling

```bash
# Profile with release build
cargo build --release
./scripts/profile_streaming.sh  # If available
```

## 🔍 Debugging

### Enable Debug Logging

```rust
// Set in your code
std::env::set_var("RUST_LOG", "debug");

// Or when running
RUST_LOG=debug cargo run --example basic_usage
```

### Common Issues

1. **Claude CLI Not Found**
   ```bash
   # Check if Claude CLI is installed
   which claude
   claude --version
   
   # If not found, install from https://claude.ai/code
   # Then add to PATH if needed:
   export PATH="$PATH:/path/to/claude"
   
   # Verify authentication
   claude auth
   ```

2. **Feature Compilation Errors**
   ```bash
   # If MCP or SQLite features fail to compile:
   cargo build                    # Use core features only
   cargo build --features cli     # Use CLI features only
   
   # Avoid these until fixed:
   # cargo build --features mcp    # Known to fail
   # cargo build --features sqlite # Known to fail
   ```

3. **Test Timeouts**
   ```bash
   # Run faster unit tests only
   cargo test --lib
   
   # Run tests with longer timeout
   cargo test --timeout 300
   
   # Skip slow integration tests during development
   cargo test --lib -- --skip integration
   ```

4. **General Compilation Issues**
   ```bash
   # Clean and rebuild
   cargo clean
   cargo build
   
   # Update dependencies
   cargo update
   
   # Check specific module
   cargo check --bin claude-sdk-rs --features cli
   ```

## 🚀 Publishing

### Pre-publish Checklist

1. **Version Bump**
   ```bash
   # Update version in Cargo.toml
   ./scripts/bump-version.sh 1.0.1  # If available
   ```

2. **Run All Checks**
   ```bash
   cargo test --all-features
   cargo clippy -- -D warnings
   cargo fmt --check
   cargo doc --all-features
   ```

3. **Test Package**
   ```bash
   cargo package --dry-run
   cargo publish --dry-run
   ```

### Publishing Process

```bash
# Use the publish script (handles dependency order)
./scripts/publish.sh

# Or manually
cargo publish
```

## 🤝 Contributing Guidelines

### 1. Code Style

- Follow Rust naming conventions
- Use `cargo fmt` before committing
- Add documentation for public APIs
- Write tests for new functionality

### 2. Commit Messages

Follow conventional commits:
```
feat: add streaming timeout configuration
fix: handle empty responses correctly
docs: update examples for new API
test: add integration tests for sessions
chore: update dependencies
```

### 3. Pull Request Process

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add/update tests
5. Update documentation
6. Submit PR with description

### 4. Review Checklist

- [ ] Tests pass (`cargo test`)
- [ ] Linter passes (`cargo clippy`)
- [ ] Code is formatted (`cargo fmt`)
- [ ] Documentation updated
- [ ] Examples work
- [ ] No breaking changes (or documented)

## 📚 Additional Resources

### Development Commands Reference

```bash
# Build commands (recommended for development)
cargo build                    # Build core SDK
cargo build --features cli     # Build with CLI
cargo build --release         # Build optimized
cargo check                    # Fast compilation check

# Test commands (recommended workflow)
cargo test --lib              # Fast unit tests only
cargo test --timeout 300      # All tests with timeout
cargo test --doc              # Test documentation examples
cargo test --features cli     # Test with CLI features

# Quality commands
cargo fmt                      # Format code
cargo clippy                   # Lint code (expect warnings)
cargo doc --no-deps           # Build documentation
cargo audit                    # Security audit (install first)

# Using Makefile (comprehensive workflow)
make help                      # Show all available commands
make dev                       # Format, lint, and test
make build                     # Standard build
make test-unit                # Unit tests only
make docs-open                # Build and open documentation

# Publishing commands
cargo package --dry-run       # Test package creation
cargo publish --dry-run       # Test publishing
./scripts/publish.sh          # Full publish script
```

**Quick Development Workflow**:
```bash
# 1. Start with core functionality
cargo build && cargo test --lib

# 2. Add CLI features when needed
cargo build --features cli

# 3. Use Makefile for comprehensive checks
make dev

# 4. Run examples to verify functionality
cargo run --example basic_usage
```

### Useful Links

- [Rust Book](https://doc.rust-lang.org/book/)
- [Async Book](https://rust-lang.github.io/async-book/)
- [Tokio Docs](https://tokio.rs/)
- [Cargo Guide](https://doc.rust-lang.org/cargo/)

## 💬 Getting Help

- **GitHub Issues**: [Report bugs](https://github.com/bredmond1019/claude-sdk-rust/issues)
- **Discussions**: [Ask questions](https://github.com/bredmond1019/claude-sdk-rust/discussions)
- **Architecture**: See [CLAUDE.md](CLAUDE.md) for technical details

---

<div align="center">

Happy coding! 🦀

</div>