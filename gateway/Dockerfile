FROM rust:1.91-slim

# Install build dependencies
# Trixie (Debian 13) provides GCC 14 (native C23) and glibc 2.40
# (stdbit.h, strlcpy) in standard repos — no workarounds needed.
# clang-19/libclang-19 are only needed for bindgen (C23 constexpr parsing).
RUN apt-get update && apt-get install -y \
    git \
    curl \
    gcc \
    g++ \
    cmake \
    pkg-config \
    libssl-dev \
    libzstd-dev \
    libhugetlbfs-dev \
    clang-19 \
    libclang-19-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /usr/src/app

# Copy all source files
COPY . .

# Point bindgen to clang-19's libclang for C23 constexpr support.
ENV LLVM_CONFIG_PATH=/usr/bin/llvm-config-19

# Build the binary
RUN cargo build --release --bin gateway

# Expose gateway port
EXPOSE 8443

# Set entrypoint with default server address for container
ENTRYPOINT ["cargo", "run", "--release", "--bin", "gateway", "--"]
CMD ["--server-addr", "0.0.0.0:8443"]
