.PHONY: all help build release test sign clean install preflight info submodules libkrun libkrunfw

PREFIX ?= /usr/local/bin
WORKSPACE_DIR := $(CURDIR)/krun-microvm

all: build sign

help:
	@echo "cro: Pure-Rust MicroVM Virtualization & Orchestration Platform"
	@echo "==============================================================="
	@echo "Usage: make [TARGET] [PREFIX=/custom/bin]"
	@echo ""
	@echo "Core Targets:"
	@echo "  make build       - Compile all SDK workspace crates (debug mode)"
	@echo "  make release     - Compile all SDK binaries with optimizations & codesign"
	@echo "  make test        - Run all 129+ unit & integration tests"
	@echo "  make sign        - Apply macOS Hypervisor.framework entitlement to binaries"
	@echo "  make install     - Install all binaries (cro, microvm, runner, shim, operator)"
	@echo "  make preflight   - Run system virtualization preflight validation"
	@echo "  make info        - Display system, hypervisor, and microVM telemetry"
	@echo "  make clean       - Remove compiled build artifacts"
	@echo ""
	@echo "Submodule Targets:"
	@echo "  make submodules  - Initialize and update libkrun and libkrunfw submodules"
	@echo "  make libkrun     - Build the libkrun C/Rust dynamic library submodule"
	@echo "  make libkrunfw   - Build the libkrunfw firmware kernel submodule"

submodules:
	git submodule update --init --recursive

build:
	$(MAKE) -C $(WORKSPACE_DIR) build

release:
	$(MAKE) -C $(WORKSPACE_DIR) release

test:
	$(MAKE) -C $(WORKSPACE_DIR) test

sign:
	$(MAKE) -C $(WORKSPACE_DIR) sign

install:
	$(MAKE) -C $(WORKSPACE_DIR) install PREFIX=$(PREFIX)

preflight:
	$(MAKE) -C $(WORKSPACE_DIR) preflight

info:
	$(MAKE) -C $(WORKSPACE_DIR) info

clean:
	$(MAKE) -C $(WORKSPACE_DIR) clean

libkrun:
	@if [ -d "libkrun" ]; then \
		echo "Building libkrun submodule..."; \
		$(MAKE) -C libkrun; \
	else \
		echo "libkrun submodule directory not found. Run 'make submodules' first."; \
		exit 1; \
	fi

libkrunfw:
	@if [ -d "libkrunfw" ]; then \
		echo "Building libkrunfw submodule..."; \
		$(MAKE) -C libkrunfw; \
	else \
		echo "libkrunfw submodule directory not found. Run 'make submodules' first."; \
		exit 1; \
	fi
