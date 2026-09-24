.PHONY: all build release test sign clean install preflight info help

PREFIX ?= /usr/local/bin
SUDO ?= $(shell if [ ! -w "$(PREFIX)" ] && [ "$$(id -u)" != "0" ]; then echo "sudo"; fi)
OS ?= $(shell uname -s)

all: build sign

help:
	@echo "libkrun-microvm Build & Installation Automation"
	@echo "==============================================="
	@echo "Targets:"
	@echo "  make build       - Build all crates in debug mode"
	@echo "  make release     - Build in release profile with LTO and code-signing"
	@echo "  make test        - Run all unit and integration tests"
	@echo "  make sign        - Apply macOS Hypervisor entitlements to debug binaries"
	@echo "  make install     - Install binaries to $$(PREFIX) (uses sudo if needed)"
	@echo "  make preflight   - Run system virtualization preflight validation"
	@echo "  make info        - Display system and microVM telemetry"
	@echo "  make clean       - Remove target build artifacts"

build:
	cargo build --workspace

release:
	cargo build --workspace --release
	@if [ "$(OS)" = "Darwin" ]; then \
		echo "Applying macOS Hypervisor.framework entitlement to release binaries..."; \
		codesign --entitlements entitlements.plist --force -s - target/release/microvm-runner; \
		codesign --entitlements entitlements.plist --force -s - target/release/microvm; \
		codesign --entitlements entitlements.plist --force -s - target/release/containerd-shim-krun-v2; \
	fi

test:
	cargo test --workspace

sign:
	@if [ "$(OS)" = "Darwin" ]; then \
		echo "Applying macOS Hypervisor.framework entitlement to debug binaries..."; \
		codesign --entitlements entitlements.plist --force -s - target/debug/microvm-runner; \
		codesign --entitlements entitlements.plist --force -s - target/debug/microvm; \
		codesign --entitlements entitlements.plist --force -s - target/debug/containerd-shim-krun-v2; \
		echo "Successfully signed binaries with Hypervisor entitlement"; \
	fi

clean:
	cargo clean

install: release
	@echo "Installing binaries to $(PREFIX)..."
	$(SUDO) install -d "$(PREFIX)"
	$(SUDO) install -m 755 target/release/microvm "$(PREFIX)/microvm"
	$(SUDO) install -m 755 target/release/microvm-runner "$(PREFIX)/microvm-runner"
	$(SUDO) install -m 755 target/release/containerd-shim-krun-v2 "$(PREFIX)/containerd-shim-krun-v2"
	$(SUDO) install -m 755 target/release/krun-operator "$(PREFIX)/krun-operator"
	@echo "All binaries successfully installed to $(PREFIX)"

preflight:
	@if [ -f target/release/microvm ]; then \
		target/release/microvm preflight; \
	elif [ -f target/debug/microvm ]; then \
		target/debug/microvm preflight; \
	else \
		$(MAKE) build && target/debug/microvm preflight; \
	fi

info:
	@if [ -f target/release/microvm ]; then \
		target/release/microvm info; \
	elif [ -f target/debug/microvm ]; then \
		target/debug/microvm info; \
	else \
		$(MAKE) build && target/debug/microvm info; \
	fi
