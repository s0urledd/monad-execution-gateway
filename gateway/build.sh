#!/usr/bin/env bash
# Build helper for native (non-Docker) environments.
#
# The Monad Execution Events SDK requires:
#   - gcc-13 (or clang-19) for compiling the C library
#   - clang-19 libclang for bindgen to parse C23 headers (constexpr)
#
# We use gcc for cmake (avoids clang-19's strict -Watomic-alignment errors)
# and clang-19's libclang for bindgen only.
#
# Prerequisites (Ubuntu 24.04):
#   wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh && sudo ./llvm.sh 19
#   sudo apt install -y libclang-19-dev
#
# Usage:
#   ./build.sh              # build only
#   ./build.sh --run        # build and run
#
set -euo pipefail

# Point bindgen to clang-19's libclang for C23 constexpr support.
# Do NOT set CC=clang-19 — let cmake use the default gcc, which avoids
# -Werror,-Watomic-alignment errors in the SDK's 128-bit atomic operations.
if command -v llvm-config-19 &>/dev/null; then
    export LLVM_CONFIG_PATH="$(command -v llvm-config-19)"
    echo "Using libclang-19 for bindgen (LLVM_CONFIG_PATH=$LLVM_CONFIG_PATH)"
    echo "Using default cc ($(cc --version | head -1)) for cmake"
elif command -v llvm-config &>/dev/null; then
    LLVM_VER="$(llvm-config --version | cut -d. -f1)"
    if [ "$LLVM_VER" -ge 19 ] 2>/dev/null; then
        export LLVM_CONFIG_PATH="$(command -v llvm-config)"
        echo "Using llvm-config version $LLVM_VER for bindgen"
    else
        echo "WARNING: llvm-config reports version $LLVM_VER (need 19+)"
        echo "Falling back to -Dconstexpr=const workaround"
        export BINDGEN_EXTRA_CLANG_ARGS="-Dconstexpr=const"
        export CFLAGS="${CFLAGS:+$CFLAGS }-Dconstexpr=const"
    fi
else
    echo "WARNING: llvm-config-19 not found"
    echo "Falling back to -Dconstexpr=const workaround"
    echo "Install: wget https://apt.llvm.org/llvm.sh && chmod +x llvm.sh && sudo ./llvm.sh 19"
    export BINDGEN_EXTRA_CLANG_ARGS="-Dconstexpr=const"
    export CFLAGS="${CFLAGS:+$CFLAGS }-Dconstexpr=const"
fi

if [[ "${1:-}" == "--run" ]]; then
    shift
    cargo run --release --bin gateway -- --server-addr 0.0.0.0:8443 "$@"
else
    cargo build --release --bin gateway "$@"
fi
