set -e

CACHE="dash-ubuntu-cache"
APT_CACHE="dash-apt-cache"
APT_LISTS_CACHE="dash-apt-lists-cache"

# Create all volumes if they don't exist
for vol in "$CACHE" "$APT_CACHE" "$APT_LISTS_CACHE"; do
    podman volume exists "$vol" || podman volume create "$vol"
done

podman run --rm -it \
    -v "$(pwd):/workspace" \
    -v "$CACHE:/cache" \
    -v "$APT_CACHE:/var/cache/apt/archives" \
    -v "$APT_LISTS_CACHE:/var/lib/apt/lists" \
    -w /workspace \
    docker.io/library/ubuntu:26.04 \
    bash -c '
    set -e
    export CARGO_HOME=/cache/cargo
    export RUSTUP_HOME=/cache/rustup
    export PATH="$RUSTUP_HOME/bin:$CARGO_HOME/bin:$PATH"

    # ---- APT: skip update if lists are fresh (< 6 hours) ----
    if [ -z "$(find /var/lib/apt/lists -type f -mmin -360 2>/dev/null | head -1)" ]; then
        apt-get update -o Acquire::CompressionTypes::Order::=gz
    fi

    # ---- APT: skip install if packages already configured ----
    if ! dpkg -l build-essential pkg-config libssl-dev curl ca-certificates 2>/dev/null \
         | grep -q "^ii  build-essential"; then
        apt-get install -y --no-install-recommends \
            build-essential pkg-config libssl-dev curl ca-certificates
    fi

    # ---- Rustup: one-time install (fully cached in volume) ----
    if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain nightly --no-modify-path
    fi
    rustup show active-toolchain >/dev/null 2>&1 || rustup default nightly
    rustup target list --installed | grep -q wasm32-unknown-unknown \
        || rustup target add wasm32-unknown-unknown

    # ---- Dioxus CLI: cache binary in volume (not host target/) ----
    DX_DIR=/cache/dx
    DX_BIN="$DX_DIR/dx"
    mkdir -p "$DX_DIR"
    if [ ! -x "$DX_BIN" ]; then
        curl -L --retry 3 -o "$DX_DIR/dx.tar.gz" \
            https://github.com/DioxusLabs/dioxus/releases/download/v0.7.3/dx-x86_64-unknown-linux-gnu.tar.gz
        tar -xf "$DX_DIR/dx.tar.gz" -C "$DX_DIR"
        rm -f "$DX_DIR/dx.tar.gz"
    fi

    # ---- Build: cargo registry, git deps, and target/ are all cached ----
    "$DX_BIN" bundle --web --release
'
