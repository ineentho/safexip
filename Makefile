.PHONY: build install-deb cross-deb clean

build:
	cargo build --release

# Build .deb natively (requires Debian/Ubuntu with cargo-deb installed)
install-deb:
	cargo deb --install

# Cross-compile .deb for Linux from macOS
cross-deb:
	scripts/cross-build-deb.sh

clean:
	cargo clean
	rm -rf target/debian
