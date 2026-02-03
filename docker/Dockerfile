# syntax=docker/dockerfile:1.4

# =============================================================================
# Stage 1: Soufflé Builder (cached separately)
# =============================================================================
FROM rustlang/rust:nightly-bookworm AS souffle-builder

RUN apt-get update && apt-get install -y \
    cmake bison flex libffi-dev libncurses-dev \
    libsqlite3-dev sqlite3 mcpp zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

ARG SOUFFLE_VERSION=2.4
RUN git clone --depth 1 --branch ${SOUFFLE_VERSION} \
    https://github.com/souffle-lang/souffle.git /tmp/souffle \
    && cd /tmp/souffle \
    && cmake -S . -B build -DCMAKE_INSTALL_PREFIX=/opt/souffle \
    && cmake --build build -j$(nproc) \
    && cmake --install build \
    && rm -rf /tmp/souffle

# =============================================================================
# Stage 2: Rust Builder (with cache mounts)
# =============================================================================
FROM rustlang/rust:nightly-bookworm AS builder

WORKDIR /app

# Install Soufflé runtime deps (needed for build.rs or tests that use it)
RUN apt-get update && apt-get install -y \
    libffi8 libncurses6 libsqlite3-0 mcpp zlib1g \
    && rm -rf /var/lib/apt/lists/*

# Copy Soufflé from builder
COPY --from=souffle-builder /opt/souffle/bin/souffle /usr/local/bin/

# Copy dependency manifests first (maximizes cache reuse)
COPY Cargo.toml Cargo.lock ./
COPY crates/winter/Cargo.toml crates/winter/
COPY crates/winter-atproto/Cargo.toml crates/winter-atproto/
COPY crates/winter-agent/Cargo.toml crates/winter-agent/
COPY crates/winter-datalog/Cargo.toml crates/winter-datalog/
COPY crates/winter-mcp/Cargo.toml crates/winter-mcp/
COPY crates/winter-scheduler/Cargo.toml crates/winter-scheduler/
COPY crates/winter-web/Cargo.toml crates/winter-web/
COPY vendor/claude-sdk-rs/Cargo.toml vendor/claude-sdk-rs/

# Create stub files so cargo can resolve the workspace
RUN mkdir -p crates/winter/src && echo "fn main() {}" > crates/winter/src/main.rs \
    && for crate in winter-atproto winter-agent winter-datalog winter-mcp winter-scheduler winter-web; do \
         mkdir -p crates/$crate/src && echo "" > crates/$crate/src/lib.rs; \
       done \
    && mkdir -p vendor/claude-sdk-rs/src && echo "" > vendor/claude-sdk-rs/src/lib.rs

# Build dependencies only (cached via mount)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release 2>/dev/null || true

# Copy full source
COPY crates/ crates/
COPY vendor/ vendor/
COPY templates/ templates/

# Touch all source files to invalidate cached compilation from stubs
RUN find crates vendor -name "*.rs" -exec touch {} \;

# Cache bust arg - pass --build-arg CACHE_BUST=$(date +%s) to force rebuild
ARG CACHE_BUST=1

# Build the actual application
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target,id=winter-target-${CACHE_BUST} \
    cargo build --release \
    && cp target/release/winter /app/winter-bin

# =============================================================================
# Stage 3: Runtime
# =============================================================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    libgomp1 \
    libffi8 \
    libncurses6 \
    zlib1g \
    mcpp \
    g++ \
    curl \
    git \
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y nodejs \
    && rm -rf /var/lib/apt/lists/*

RUN npm install -g @anthropic-ai/claude-code \
    && ln -sf "$(npm bin -g)/claude" /usr/local/bin/claude

# Install Deno for custom tool sandboxing
COPY --from=denoland/deno:bin-2.1.4 /deno /usr/local/bin/deno

COPY --from=souffle-builder /opt/souffle/bin/souffle /usr/local/bin/
COPY --from=builder /app/winter-bin /usr/local/bin/winter

RUN useradd -m -s /bin/bash winter
RUN mkdir -p /home/winter/.config/winter
COPY config/mcp.json /home/winter/.config/winter/mcp.json
# Create empty secrets file with proper permissions
RUN echo '{"version":1,"secrets":{}}' > /home/winter/.config/winter/secrets.json \
    && chmod 600 /home/winter/.config/winter/secrets.json
RUN chown -R winter:winter /home/winter/.config

USER winter
WORKDIR /home/winter

CMD ["winter", "daemon"]
