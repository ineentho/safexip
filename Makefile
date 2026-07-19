VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
PACKAGE_ARCH ?= amd64
export VERSION
export PACKAGE_ARCH

.PHONY: build check test package package-deb package-rpm package-arch check-linux clean

build:
	cargo build --release --locked

test:
	cargo test --all-targets --locked

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	cargo test --all-targets --locked

check-linux:
	@test "$$(uname -s)" = Linux || { echo "error: Linux packages must be built on Linux" >&2; exit 1; }

package: check-linux build
	mkdir -p dist
	nfpm package --packager deb --target dist/
	nfpm package --packager rpm --target dist/
	nfpm package --packager archlinux --target dist/

package-deb: check-linux build
	mkdir -p dist
	nfpm package --packager deb --target dist/

package-rpm: check-linux build
	mkdir -p dist
	nfpm package --packager rpm --target dist/

package-arch: check-linux build
	mkdir -p dist
	nfpm package --packager archlinux --target dist/

clean:
	cargo clean
	rm -rf dist
