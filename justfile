# Run checks for both server and web
check:
    cargo clippy --features server -Zno-index-update
    cargo clippy --features web -Zno-index-update

# Serve website
serve:
	dx serve --no-default-features --web

# Format code
fmt:
	cargo fmt
	dx fmt
	tombi fmt Cargo.toml
