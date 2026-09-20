.PHONY: all build test sign clean install

all: build sign

build:
	cargo build --workspace

release:
	cargo build --workspace --release
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		codesign --entitlements entitlements.plist --force -s - target/release/microvm-runner; \
	fi

test:
	cargo test --workspace

sign:
	@if [ "$$(uname -s)" = "Darwin" ]; then \
		codesign --entitlements entitlements.plist --force -s - target/debug/microvm-runner; \
		echo "Signed target/debug/microvm-runner with Hypervisor entitlement"; \
	fi

clean:
	cargo clean

install: release
	install -d /usr/local/bin
	install -m 755 target/release/microvm /usr/local/bin/microvm
	install -m 755 target/release/microvm-runner /usr/local/bin/microvm-runner
	install -m 755 target/release/containerd-shim-krun-v2 /usr/local/bin/containerd-shim-krun-v2
