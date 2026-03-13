# Run checks for both server and web
check:
    cargo clippy --features server
    cargo clippy --features web

# Serve website
serve:
	dx serve --web

# Format code
fmt:
	cargo fmt
	dx fmt
