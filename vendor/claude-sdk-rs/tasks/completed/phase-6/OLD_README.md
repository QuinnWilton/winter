# claude-ai 🦀

[![Crates.io](https://img.shields.io/crates/v/claude-ai.svg)](https://crates.io/crates/claude-ai)
[![Documentation](https://docs.rs/claude-ai/badge.svg)](https://docs.rs/claude-ai)
[![CI](https://github.com/frgmt0/claude-ai/workflows/CI/badge.svg)](https://github.com/frgmt0/claude-ai/actions)
[![codecov](https://codecov.io/gh/frgmt0/claude-ai/branch/main/graph/badge.svg)](https://codecov.io/gh/frgmt0/claude-ai)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)

A type-safe, async-first Rust SDK for [Claude Code](https://github.com/anthropics/claude-code) that transforms the CLI tool into a powerful programmatic API for building AI-powered applications.

## ✨ Features

- **🔒 Type Safety** - Strongly typed requests/responses with compile-time guarantees
- **⚡ Async/Await** - Built on Tokio for efficient concurrent operations
- **📊 Rich Metadata** - Access costs, tokens, timing, and raw JSON responses
- **🔄 Streaming** - Real-time response processing with async iterators
- **💾 Session Management** - Persistent conversations with context preservation
- **🛠️ Tool Integration** - Model Context Protocol (MCP) support for external tools
- **🎯 Error Handling** - Comprehensive error types with actionable messages
- **⚙️ Flexible Config** - Builder patterns for all configuration options

## 🚀 Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
claude-ai = "1.0.0"
tokio = { version = "1.0", features = ["full"] }
```

Basic usage:

```rust
use claude_ai::{Client, Config};

#[tokio::main]
async fn main() -> Result<(), claude_ai::Error> {
    let client = Client::new(Config::default());

    let response = client
        .query("Explain Rust ownership in simple terms")
        .send()
        .await?;

    println!("{}", response);
    Ok(())
}
```

## 📚 Documentation

- **[📖 Quick Start Guide](QUICK_START.md)** - Get up and running in minutes
- **[🎓 Tutorial](TUTORIAL.md)** - In-depth guide with advanced examples
- **[💡 Future Ideas](FUTURE_IDEAS.md)** - Roadmap and feature ideas
- **[📋 API Docs](https://docs.rs/claude-ai)** - Complete API reference
- **[❓ FAQ](FAQ.md)** - Frequently asked questions

## 🏗️ Architecture

The SDK is built with a modular architecture:

```
claude-ai/                 # 🎯 Main SDK - Public API facade
├── claude-ai-core/        # 🧱 Core types, errors, configuration
├── claude-ai-runtime/     # 🚀 Process execution and streaming
├── claude-ai-mcp/         # 🔌 Model Context Protocol integration
└── claude-ai-interactive/ # 🖥️ Interactive terminal interface
```

### Key Design Principles

- **Type Safety First** - Catch errors at compile time, not runtime
- **Async Native** - Built for modern concurrent Rust applications
- **Zero-Cost Abstractions** - Minimal overhead over direct CLI usage
- **Extensible** - Plugin architecture for tools and integrations

## 🎯 Common Use Cases

- **AI-Powered CLI Tools** - Build intelligent command-line applications
- **Web Applications** - Add AI features to web services and APIs
- **Data Processing** - Batch process content with AI analysis
- **Development Tools** - Create AI-assisted development workflows
- **Research & Experimentation** - Prototype AI features quickly

## 📋 Prerequisites

- **Rust 1.70+** - Modern async/await support required
- **Claude Code CLI** - Install and authenticate with Claude
  ```bash
  # Install Claude CLI (see official docs for instructions)
  claude auth
  ```

## 🔧 Configuration Examples

### Builder Pattern

```rust
use claude_ai::{Client, StreamFormat};

let client = Client::builder()
    .model("claude-sonnet-4-20250514")
    .system_prompt("You are a Rust expert assistant")
    .stream_format(StreamFormat::Json)
    .timeout_secs(60)
    .allowed_tools(vec!["filesystem".to_string()])
    .build();
```

### Advanced Features

#### Session Management

```rust
// Sessions are managed automatically - context is preserved across queries
let client = Client::new(Config::default());

// First query creates a session
let response1 = client.query("My name is Alice").send().await?;

// Subsequent queries use the same session
let response2 = client.query("What's my name?").send().await?;
// Claude will remember "Alice"

// Access session ID from metadata
let response = client.query("Hello").send_full().await?;
println!("Session ID: {}", response.metadata?.session_id);
```

#### Response with Metadata

```rust
// Get full response with metadata
let response = client.query("Analyze this code").send_full().await?;
println!("Cost: ${:.6}", response.metadata?.cost_usd.unwrap_or(0.0));
println!("Tokens used: {:?}", response.metadata?.tokens_used);
println!("Model: {}", response.metadata?.model);
```

#### Streaming Responses

```rust
// Stream responses in real-time
use futures::StreamExt;

let mut stream = client.query("Write a story").stream().await?;
while let Some(chunk) = stream.next().await {
    print!("{}", chunk?);
}
```

#### Error Handling

```rust
use claude_ai::Error;

match client.query("Hello").send().await {
    Ok(response) => println!("Success: {}", response),
    Err(Error::Timeout) => eprintln!("Request timed out"),
    Err(Error::ClaudeNotAuthenticated) => eprintln!("Please run: claude auth"),
    Err(Error::ClaudeNotFound) => eprintln!("Claude CLI not installed"),
    Err(Error::ProcessFailed(code, stderr)) => eprintln!("Process error {}: {}", code, stderr),
    Err(e) => eprintln!("Other error: {:?}", e),
}
```

#### Tool Permissions

```rust
// Enable specific tools using MCP format
let client = Client::builder()
    .allowed_tools(vec![
        "mcp__filesystem__read".to_string(),    // MCP filesystem read
        "mcp__filesystem__write".to_string(),   // MCP filesystem write
        "bash:ls".to_string(),                  // Bash command
    ])
    .build();
```

## 🧪 Examples

Check out the [`examples/`](claude-ai/examples/) directory for complete working examples:

- [`basic.rs`](claude-ai/examples/basic.rs) - Simple query and response
- [`streaming.rs`](claude-ai/examples/streaming.rs) - Real-time streaming
- [`with_tools.rs`](claude-ai/examples/with_tools.rs) - Tool integration
- [`raw_json.rs`](claude-ai/examples/raw_json.rs) - Full JSON access

Run examples with:

```bash
cargo run --example basic
```

## 🔧 Troubleshooting

### Common Issues

**Claude CLI not found**

```bash
# Error: "Claude binary not found"
# Solution: Install Claude CLI
curl -fsSL https://claude.ai/install.sh | sh
```

**Authentication required**

```bash
# Error: "Not authenticated"
# Solution: Authenticate with your API key
claude auth
```

**Timeout errors**

```rust
// Increase timeout for long operations
let client = Client::builder()
    .timeout_secs(120)  // 2 minutes
    .build();
```

**Session context issues**

```rust
// Ensure you're reusing the same client instance
let client = Client::new(Config::default());  // Create once
// Use `client` for all queries in the conversation
```

For more detailed troubleshooting, see the [FAQ](FAQ.md) or [open an issue](https://github.com/frgmt0/claude-ai/issues).

## 📊 Project Metrics

_Last verified: 2025-06-17_

- **Project Health**: 8.5/10
- **Test Coverage**: 84 comprehensive tests across all crates
- **Test Pass Rate**: 100%
- **Crate Count**: 5 production-ready crates
- **Lines of Code**: ~15,000+ (excluding tests)
- **Platform Support**: Linux, macOS, Windows (x86_64, ARM64)

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details.

### Development Commands

```bash
cargo build          # Build all crates
cargo test           # Run tests
cargo clippy         # Run linter
cargo fmt            # Format code
./scripts/publish.sh # Publish to crates.io
```

## 📜 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🔗 Related Projects

- **[Claude Code](https://github.com/anthropics/claude-code)** - Official Claude CLI tool
- **[Anthropic API](https://docs.anthropic.com/)** - Direct API access
- **[Claude Chat](https://claude.ai/)** - Web interface for Claude

---

<div align="center">

**[📖 Quick Start](QUICK_START.md)** • **[🎓 Tutorial](TUTORIAL.md)** • **[💡 Future Ideas](FUTURE_IDEAS.md)** • **[📋 API Docs](https://docs.rs/claude-ai)** • **[❓ FAQ](FAQ.md)**

Made with ❤️ for the Rust community

</div>
