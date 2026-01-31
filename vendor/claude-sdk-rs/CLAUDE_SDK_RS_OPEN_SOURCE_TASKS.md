# claude-sdk-rs Open Source Readiness Tasks

Streamlined task list for rebranding to `claude-sdk-rs` and consolidating into a single crate for simplified open-source release.

## 🚨 PHASE 1: CRITICAL BLOCKERS (Must complete before any public release)

### 1. Rebrand to claude-sdk-rs [2 hours]

- [ ] Rename project from `claude-sdk-rs` to `claude-sdk-rs`
- [ ] Update root Cargo.toml name field
- [ ] Update all README.md references to new name
- [ ] Update CONTRIBUTING.md, SECURITY.md, and other docs
- [ ] Search/replace "claude-sdk-rs" → "claude-sdk-rs" throughout codebase
- [ ] Update GitHub repository name (if applicable)

### 2. Consolidate to Single Crate with Feature Flags [4 hours]

- [ ] Merge `claude-sdk-rs-core/` into main `src/` directory
- [ ] Merge `claude-sdk-rs-runtime/` into main `src/` directory
- [ ] Merge `claude-sdk-rs-mcp/` into main `src/` directory
- [ ] Move `claude-sdk-rs-interactive/` into main crate behind feature flags:
  - [ ] Create `cli` feature flag for CLI functionality
  - [ ] Create `analytics` feature flag for dashboard/analytics
  - [ ] Create `full` feature flag that enables everything
- [ ] Update main Cargo.toml to include all dependencies with proper feature gating
- [ ] Remove workspace configuration - make it a single crate
- [ ] Update module declarations in lib.rs with conditional compilation
- [ ] Test that `cargo build` works for default features
- [ ] Test that `cargo build --all-features` includes CLI

### 3. Fix Repository Links [30 minutes]

- [ ] Update repository field in Cargo.toml to correct GitHub URL: `https://github.com/frgmt0/claude-sdk-rs`
- [ ] Fix all documentation links from `frgmt0/claude-sdk-rs` to actual repo
- [ ] Update homepage and documentation URLs

### 4. Fix Compilation Errors [1.5 hours]

- [ ] Fix the 20 compilation errors found in tests
- [ ] Ensure `cargo test` passes completely
- [ ] Ensure `cargo test --all-features` passes
- [ ] Fix clippy errors and warnings

## 🔧 PHASE 2: HIGH PRIORITY FIXES (For functional release)

### 5. Legal & Compliance [1 hour]

- [ ] Create CODE_OF_CONDUCT.md (use Contributor Covenant)
- [ ] Update copyright holder from "coldie" to proper entity
- [ ] Add basic CLA or DCO requirement to CONTRIBUTING.md
- [ ] Update security contact in SECURITY.md

### 6. Clean Up Dependencies [1.5 hours]

- [ ] Remove all path dependencies (no longer needed with single crate)
- [ ] Update `dotenv` to `dotenvy`
- [ ] Run `cargo audit` and fix any security issues
- [ ] Optimize dependency list - use feature flags to make heavy deps optional
- [ ] Move CLI-specific dependencies behind `cli` feature flag

### 7. Essential Documentation [2.5 hours]

- [ ] Create single comprehensive README.md for the consolidated crate
- [ ] Include clear installation instructions:
  - [ ] Basic SDK: `cargo add claude-sdk-rs`
  - [ ] With CLI: `cargo add claude-sdk-rs --features cli`
  - [ ] Everything: `cargo add claude-sdk-rs --features full`
- [ ] Add quick start examples for both SDK and CLI usage
- [ ] Create ARCHITECTURE.md explaining feature flag structure
- [ ] Document how to install CLI binary: `cargo install claude-sdk-rs --features cli`
- [ ] Enhance .gitignore with comprehensive patterns

## 🏗️ PHASE 3: PUBLISHING PREPARATION (For quality release)

### 8. Cargo.toml Metadata [45 minutes]

- [ ] Add comprehensive description mentioning both SDK and CLI capabilities
- [ ] Add relevant keywords ("claude", "ai", "sdk", "cli", "anthropic", "llm")
- [ ] Add categories (e.g., "api-bindings", "command-line-utilities")
- [ ] Set license = "MIT"
- [ ] Add documentation and homepage URLs
- [ ] Set minimum Rust version (MSRV)
- [ ] Configure feature flags properly:
  ```toml
  [features]
  default = []
  cli = ["clap", "tokio/full", "colored", ...cli deps...]
  analytics = ["dep:prettytable", ...analytics deps...]
  mcp = [...mcp deps...]
  full = ["cli", "analytics", "mcp"]
  ```

### 9. CI/CD Setup [2.5 hours]

- [ ] Update GitHub Actions for single crate with feature matrix
- [ ] Test default features, cli features, and all features separately
- [ ] Remove continue-on-error from test jobs
- [ ] Add `cargo publish --dry-run` to CI
- [ ] Create release workflow that can build CLI binaries
- [ ] Add basic security scanning

### 10. Examples & Documentation [2.5 hours]

- [ ] Create `examples/` directory with use cases:
  - SDK examples:
    - [ ] `basic_usage.rs`
    - [ ] `streaming.rs`
    - [ ] `error_handling.rs`
    - [ ] `configuration.rs`
    - [ ] `session_management.rs`
  - CLI examples (with `required-features = ["cli"]`):
    - [ ] `cli_interactive.rs`
    - [ ] `cli_analytics.rs`
- [ ] Ensure all examples compile and run
- [ ] Add rustdoc documentation to public APIs
- [ ] Document feature flags in lib.rs

## 📁 PROJECT STRUCTURE WITH FEATURES

After consolidation, the project should look like:

```
claude-sdk-rs/
├── Cargo.toml (single crate with feature flags)
├── README.md
├── LICENSE
├── CODE_OF_CONDUCT.md
├── CONTRIBUTING.md
├── SECURITY.md
├── src/
│   ├── lib.rs
│   ├── client.rs (core SDK)
│   ├── config.rs (core SDK)
│   ├── error.rs (core SDK)
│   ├── session/ (core SDK)
│   ├── process.rs (core SDK)
│   ├── mcp/ (behind 'mcp' feature)
│   ├── cli/ (behind 'cli' feature)
│   │   ├── mod.rs
│   │   ├── commands.rs
│   │   └── ...
│   ├── analytics/ (behind 'analytics' feature)
│   │   ├── dashboard.rs
│   │   └── ...
│   └── ...
├── examples/
│   ├── basic_usage.rs
│   ├── cli_demo.rs (requires 'cli' feature)
│   └── ...
├── tests/
│   └── integration_tests.rs
└── .github/
    └── workflows/
        ├── ci.yml (tests all feature combinations)
        └── release.yml
```

## 📊 EFFORT ESTIMATE

**Phase 1 (Critical):** ~8 hours (added 1 hour for feature flag setup)
**Phase 2 (High Priority):** ~5 hours  
**Phase 3 (Publishing):** ~5.5 hours

**Total: ~18.5 hours** (slightly more than original 16 due to feature flag complexity)

## 🎯 BENEFITS OF FEATURE FLAG APPROACH

1. **Preserves All Work** - No functionality is lost, everything is retained
2. **Flexible Usage** - Users can choose minimal SDK or full CLI experience
3. **Lighter Default** - SDK-only users get a minimal dependency footprint
4. **Binary Distribution** - Can still distribute CLI via `cargo install`
5. **Best of Both Worlds** - Simple SDK API with optional rich CLI features

## 🚀 USAGE PATTERNS

### As SDK Only (minimal dependencies):

```toml
[dependencies]
claude-sdk-rs = "0.1.0"
```

### As CLI Tool (full interactive experience):

```bash
cargo install claude-sdk-rs --features cli
```

### As Library with Analytics:

```toml
[dependencies]
claude-sdk-rs = { version = "0.1.0", features = ["analytics"] }
```

### Everything Enabled:

```toml
[dependencies]
claude-sdk-rs = { version = "0.1.0", features = ["full"] }
```

## 🚀 RELEASE STRATEGY

1. **v0.1.0-beta**: After Phase 1 completion (basic functionality)
2. **v0.1.0**: After Phase 2 completion (production ready)
3. **v0.2.0+**: After Phase 3 completion (full featured)

## ✅ CHECKLIST FOR OPEN SOURCE RELEASE

Before publishing to crates.io:

- [ ] All tests pass: `cargo test --all-features`
- [ ] No clippy warnings: `cargo clippy --all-features`
- [ ] Documentation builds: `cargo doc --all-features`
- [ ] Examples work: `cargo run --example basic_usage`
- [ ] CLI works: `cargo run --features cli -- --help`
- [ ] Dry run succeeds: `cargo publish --dry-run`
- [ ] Legal review complete (no trademark issues)
- [ ] Security audit clean
- [ ] README clearly explains feature flags

**Minimum viable open source release:** Complete Phase 1 + items 5-7 from Phase 2 (~11 hours total)
