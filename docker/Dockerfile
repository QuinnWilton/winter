# syntax=docker/dockerfile:1.4

# =============================================================================
# Stage 1: Soufflé Builder (cached separately)
# =============================================================================
FROM debian:bookworm-slim AS souffle-builder

RUN apt-get update && apt-get install -y \
    cmake bison flex libffi-dev libncurses-dev \
    libsqlite3-dev sqlite3 mcpp zlib1g-dev \
    git g++ python3 \
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
# Stage 2: Chef - Install cargo-chef
# =============================================================================
FROM rustlang/rust:nightly-bookworm AS chef

RUN cargo install cargo-chef
WORKDIR /app

# =============================================================================
# Stage 3: Planner - Generate dependency recipe
# =============================================================================
FROM chef AS planner

COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# Stage 4: Builder - Cook dependencies, then build
# =============================================================================
FROM chef AS builder

# Install Soufflé runtime deps and mold linker for faster builds
RUN apt-get update && apt-get install -y \
    libffi8 libncurses6 libsqlite3-0 mcpp zlib1g \
    mold \
    && rm -rf /var/lib/apt/lists/*

# Copy Soufflé from builder
COPY --from=souffle-builder /opt/souffle/bin/souffle /usr/local/bin/

# Copy cargo config for mold linker
COPY .cargo/config.toml .cargo/

# Copy recipe and cook dependencies (cached layer)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy full source and build
COPY . .
RUN cargo build --release --bin winter

# =============================================================================
# Stage 5: Runtime
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
COPY --from=builder /app/target/release/winter /usr/local/bin/winter

RUN useradd -m -s /bin/bash winter
RUN mkdir -p /home/winter/.config/winter /home/winter/.cache/deno /home/winter/workspace
COPY config/mcp.json /home/winter/.config/winter/mcp.json
# Create empty secrets file with proper permissions
RUN echo '{"version":1,"secrets":{}}' > /home/winter/.config/winter/secrets.json \
    && chmod 600 /home/winter/.config/winter/secrets.json
RUN chown -R winter:winter /home/winter/.config /home/winter/.cache /home/winter/workspace

USER winter
WORKDIR /home/winter

CMD ["winter", "daemon"]
