set -e

CACHE="dash-ubuntu-cache"
APT_CACHE="dash-apt-cache"

# Create both volumes if they don't exist
podman volume exists "$CACHE"     || podman volume create "$CACHE"
podman volume exists "$APT_CACHE" || podman volume create "$APT_CACHE"

podman run --rm -it \
    -v "$(pwd):/workspace" \
    -v "$CACHE:/cache" \
    -v "$APT_CACHE:/var/cache/apt/archives" \
    -w /workspace \
    docker.io/library/ubuntu:26.04 \
    bash -c "
    set -e
    export CARGO_HOME=/cache/cargo
    export RUSTUP_HOME=/cache/rustup
    export PATH=\"\$RUSTUP_HOME/bin:\$CARGO_HOME/bin:\$PATH\"

    mkdir -p ~/.cargo
    cat > ~/.cargo/config.toml << 'EOF'
        [http]
        check-revoke = false
        ssl-verify = false
EOF

    # One-time Rust setup (cached in volume)
    if ! command -v cargo &> /dev/null; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
        rustup target add wasm32-unknown-unknown
    fi

    # hopefully fast - should reuse cache
    apt-get update
    apt-get install -y build-essential pkg-config libssl-dev curl ca-certificates

    if [ ! -f target/ubuntu-build/dx ]; then
	    curl -L https://github.com/DioxusLabs/dioxus/releases/download/v0.7.3/dx-x86_64-unknown-linux-gnu.tar.gz > target/ubuntu-build/dx.tar.gz
	    tar -xf target/ubuntu-build/dx.tar.gz
    fi

    ./target/ubuntu-build/dx bundle --web --release
"
