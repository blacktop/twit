set shell := ["bash", "-cu"]

# Default recipe - show help
default:
    @just --list

# Build debug
build:
    cargo build

# Build release
release:
    cargo build --release

# Run tests
test:
    RUST_BACKTRACE=1 cargo test

# Clean build artifacts
clean:
    cargo clean

# Format code
fmt:
    cargo fmt --all

# Lint with clippy
lint:
    cargo clippy -- -D warnings

# Build and open docs
doc:
    cargo doc --no-deps --open

# Update dependencies
update:
    cargo update

# Run benchmarks
bench:
    cargo bench

# Record terminal demo
rec:
    @echo "📼 Recording"
    t-rec --profile demo

# Tail application logs
logs:
    tail -f ~/Library/Caches/io.blacktop.twit/twit.log

# Create a new version tag and push (usage: just tag 0.1.0)
tag version:
    @echo "Creating tag v{{ version }}..."
    git tag -a "v{{ version }}" -m "Release v{{ version }}"
    git push origin "v{{ version }}"
    @echo "Tag v{{ version }} pushed. GitHub Actions will handle the release."

# Show current version from Cargo.toml
version:
    @grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/'

# Build snapshot with goreleaser
snapshot:
    goreleaser build --clean --timeout 60m --snapshot --single-target --output dist/twit

# Build and publish release with goreleaser
dist: bump-patch
    goreleaser release --clean --timeout 60m --skip=validate
    # @bash -c 'set -euo pipefail; version="$$(just version)"; cp dist/homebrew/Casks/twit.rb ../homebrew-tap/Casks/twit.rb; cd ../homebrew-tap; git add Casks/twit.rb; git commit -m "Bump twit to version $$version"; git push'

# Cross-build Linux binaries on macOS using cargo-zigbuild (requires Zig + cargo-zigbuild)
zigbuild target="x86_64-unknown-linux-gnu":
    cargo zigbuild --release --target {{ target }} --no-default-features

# Bump patch version, commit, tag, and push (requires cargo-release: cargo install cargo-release)
bump: bump-patch

# Bump patch version (0.1.0 -> 0.1.1)
bump-patch:
    cargo release patch --execute --no-publish

# Bump minor version (0.1.0 -> 0.2.0)
bump-minor:
    cargo release minor --execute --no-publish

# Bump major version (0.1.0 -> 1.0.0)
bump-major:
    cargo release major --execute --no-publish

# Preview what bump would do (dry-run)
bump-dry level="patch":
    cargo release {{ level }}
