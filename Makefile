VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
PACKAGE_ARCH ?= amd64
export VERSION
export PACKAGE_ARCH

.PHONY: build package package-deb package-rpm package-apk package-arch clean

build:
	cargo build --release

package: build
	mkdir -p dist
	nfpm package --packager deb --target dist/
	nfpm package --packager rpm --target dist/
	nfpm package --packager apk --target dist/
	nfpm package --packager archlinux --target dist/

package-deb: build
	mkdir -p dist
	nfpm package --packager deb --target dist/

package-rpm: build
	mkdir -p dist
	nfpm package --packager rpm --target dist/

package-apk: build
	mkdir -p dist
	nfpm package --packager apk --target dist/

package-arch: build
	mkdir -p dist
	nfpm package --packager archlinux --target dist/

clean:
	cargo clean
	rm -rf dist
