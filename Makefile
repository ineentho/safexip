VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
PACKAGE_ARCH ?= amd64
PACKAGE_TARGET ?= x86_64-unknown-linux-musl
export VERSION
export PACKAGE_ARCH

.PHONY: build build-package check test package package-deb package-rpm package-apk package-arch check-linux clean

build:
	cargo build --release --locked

build-package:
	cargo build --release --locked --target $(PACKAGE_TARGET)

test:
	cargo test --all-targets --locked

check:
	cargo fmt --all -- --check
	cargo clippy --all-targets --all-features --locked -- -D warnings
	cargo test --all-targets --locked

check-linux:
	@test "$$(uname -s)" = Linux || { echo "error: Linux packages must be built on Linux" >&2; exit 1; }

package: check-linux build-package
	mkdir -p dist
	nfpm package --packager deb --target dist/
	nfpm package --packager rpm --target dist/
	nfpm package --packager apk --target dist/
	nfpm package --packager archlinux --target dist/

package-deb: check-linux build-package
	mkdir -p dist
	nfpm package --packager deb --target dist/

package-rpm: check-linux build-package
	mkdir -p dist
	nfpm package --packager rpm --target dist/

package-apk: check-linux build-package
	mkdir -p dist
	nfpm package --packager apk --target dist/

package-arch: check-linux build-package
	mkdir -p dist
	nfpm package --packager archlinux --target dist/

clean:
	cargo clean
	rm -rf dist
