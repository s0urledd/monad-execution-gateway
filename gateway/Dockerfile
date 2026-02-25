FROM rust:1.91-slim-bookworm

# Install build dependencies
# The Monad Execution Events SDK requires clang-19+ for C23 constexpr support.
# We add the LLVM apt repository to get clang-19.
RUN apt-get update && apt-get install -y \
    git \
    curl \
    wget \
    gnupg \
    gcc \
    g++ \
    cmake \
    pkg-config \
    libssl-dev \
    libzstd-dev \
    libhugetlbfs-dev \
    libbsd-dev \
    software-properties-common \
    && wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc \
    && echo "deb http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-19 main" > /etc/apt/sources.list.d/llvm-19.list \
    && apt-get update \
    && apt-get install -y clang-19 libclang-19-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /usr/src/app

# Copy all source files
COPY . .

# Point bindgen to clang-19's libclang for C23 constexpr support.
# GCC 12 (Bookworm) does not support C23, so we must use clang-19 as the
# C compiler.  -Wno-error=atomic-alignment suppresses the 128-bit atomic
# alignment warning that clang-19 emits with -Werror.
ENV LLVM_CONFIG_PATH=/usr/bin/llvm-config-19
ENV CC=clang-19
ENV CFLAGS="-Wno-error=atomic-alignment -include bsd/string.h"
ENV RUSTFLAGS="-C link-arg=-lbsd"

# Build the binary
RUN cargo build --release --bin gateway

# Expose gateway port
EXPOSE 8443

# Set entrypoint with default server address for container
ENTRYPOINT ["cargo", "run", "--release", "--bin", "gateway", "--"]
CMD ["--server-addr", "0.0.0.0:8443"]
