#!/usr/bin/env bash
# ==============================================================================
# libkrun-microvm Automated Installation Script
# Supports: macOS (Darwin Apple Silicon / Intel) & Linux (Ubuntu/Debian, Fedora/RHEL, Arch)
# ==============================================================================

set -euo pipefail

# --- Color Formatting ---
BOLD='\033[1m'
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

log_info() {
    printf "${BLUE}${BOLD}[INFO]${NC} %s\n" "$1"
}

log_success() {
    printf "${GREEN}${BOLD}[OK]${NC} %s\n" "$1"
}

log_warn() {
    printf "${YELLOW}${BOLD}[WARN]${NC} %s\n" "$1"
}

log_error() {
    printf "${RED}${BOLD}[ERROR]${NC} %s\n" "$1" >&2
}

log_step() {
    printf "\n${CYAN}${BOLD}==> %s${NC}\n" "$1"
}

# --- Determine Script and Workspace Directory ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "$SCRIPT_DIR/Cargo.toml" ] && grep -q "krun-microvm" "$SCRIPT_DIR/Cargo.toml" 2>/dev/null; then
    WORKSPACE_DIR="$SCRIPT_DIR"
elif [ -d "$SCRIPT_DIR/krun-microvm" ]; then
    WORKSPACE_DIR="$SCRIPT_DIR/krun-microvm"
else
    WORKSPACE_DIR="$SCRIPT_DIR"
fi

cd "$WORKSPACE_DIR"

# --- Default Options ---
BUILD_MODE="release"
DO_BUILD=true
SKIP_DEPS=false
AUTO_YES=false
DO_SIGN=true
PREFIX=""

# --- Usage Banner ---
print_help() {
    printf "%b" "${BOLD}libkrun-microvm Installer${NC}
Automated prerequisite checker, builder, code-signer, and installer for libkrun microVMs.

${BOLD}USAGE:${NC}
    ./install.sh [OPTIONS]

${BOLD}OPTIONS:${NC}
    --prefix <DIR>       Destination directory for binaries (default: /usr/local/bin or ~/.local/bin)
    --debug              Build with debug profile instead of release
    --no-build           Skip cargo build (use already compiled binaries from target/)
    --skip-deps          Skip checking/installing system dependencies (libkrun, rust, etc.)
    --no-sign            Skip macOS codesign with Hypervisor entitlements
    -y, --yes            Automatic yes to prompts (non-interactive mode)
    -h, --help           Show this help message and exit

${BOLD}BINARIES INSTALLED:${NC}
    - microvm                     Core CLI tool for running OCI images & sandboxes
    - microvm-runner              MicroVM supervisor and init launcher
    - containerd-shim-krun-v2     Containerd v2 runtime shim for Kubernetes
    - krun-operator               Pure-Rust Kubernetes Operator (kube-rs)

"
}

# --- Parse Arguments ---
while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --debug)
            BUILD_MODE="debug"
            shift
            ;;
        --no-build)
            DO_BUILD=false
            shift
            ;;
        --skip-deps)
            SKIP_DEPS=true
            shift
            ;;
        --no-sign)
            DO_SIGN=false
            shift
            ;;
        -y|--yes)
            AUTO_YES=true
            shift
            ;;
        -h|--help)
            print_help
            exit 0
            ;;
        *)
            log_error "Unknown option: $1"
            print_help
            exit 1
            ;;
    esac
done

# --- Determine Install Prefix ---
if [ -z "$PREFIX" ]; then
    if [ "$(id -u)" -eq 0 ]; then
        PREFIX="/usr/local/bin"
    elif [ -w "/usr/local/bin" ]; then
        PREFIX="/usr/local/bin"
    else
        PREFIX="$HOME/.local/bin"
    fi
fi

# --- System Detection ---
OS="$(uname -s)"
ARCH="$(uname -m)"

printf "${BOLD}======================================================${NC}\n"
printf "${CYAN}${BOLD}     libkrun-microvm System Installation Tool        ${NC}\n"
printf "${BOLD}======================================================${NC}\n"
printf "OS:                  ${BOLD}%s${NC}\n" "$OS"
printf "Architecture:        ${BOLD}%s${NC}\n" "$ARCH"
printf "Workspace:           ${BOLD}%s${NC}\n" "$WORKSPACE_DIR"
printf "Target Directory:    ${BOLD}%s${NC}\n" "$PREFIX"
printf "Build Profile:       ${BOLD}%s${NC}\n" "$BUILD_MODE"
printf "${BOLD}======================================================${NC}\n\n"

# ==============================================================================
# 1. PREREQUISITE CHECK & INSTALLATION
# ==============================================================================
if [ "$SKIP_DEPS" = false ]; then
    log_step "Checking System Virtualization & Prerequisites"

    # --------------------------------------------------------------------------
    # macOS (Darwin)
    # --------------------------------------------------------------------------
    if [ "$OS" = "Darwin" ]; then
        log_info "Detected macOS Darwin ($ARCH)"

        # Check Apple Silicon
        if [ "$ARCH" != "arm64" ]; then
            log_warn "Running on non-ARM64 architecture ($ARCH). Apple Silicon (M1/M2/M3/M4) is strongly recommended for full GPU and Hypervisor acceleration."
        else
            log_success "Apple Silicon architecture confirmed (Hypervisor.framework ready)."
        fi

        # Check Homebrew
        if ! command -v brew &>/dev/null; then
            log_error "Homebrew is required on macOS to install libkrun and libkrunfw."
            printf "Install Homebrew via: /bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\"\n"
            exit 1
        else
            log_success "Homebrew detected: $(brew --version | head -n 1)"
        fi

        # Check libkrun & libkrunfw
        LIBKRUN_FOUND=false
        if [ -f "/opt/homebrew/lib/libkrun.dylib" ] || [ -f "/usr/local/lib/libkrun.dylib" ] || brew list libkrun &>/dev/null; then
            LIBKRUN_FOUND=true
        fi

        if [ "$LIBKRUN_FOUND" = true ]; then
            log_success "libkrun and libkrunfw libraries are installed."
        else
            log_info "libkrun / libkrunfw not found. Installing via Homebrew tap slp/krun..."
            brew tap slp/krun
            brew install libkrun libkrunfw
            log_success "libkrun and libkrunfw installed successfully via Homebrew."
        fi

    # --------------------------------------------------------------------------
    # Linux
    # --------------------------------------------------------------------------
    elif [ "$OS" = "Linux" ]; then
        log_info "Detected Linux ($ARCH)"

        # Check /dev/kvm Hardware Virtualization
        if [ -e "/dev/kvm" ]; then
            log_success "Hardware virtualization device (/dev/kvm) detected."
            if [ ! -r "/dev/kvm" ] || [ ! -w "/dev/kvm" ]; then
                log_warn "Current user does not have read/write permissions for /dev/kvm."
                log_info "You may need to run: sudo usermod -aG kvm \$USER (then log out and back in)."
            fi
        else
            log_warn "/dev/kvm not found. Checking if KVM kernel modules are loaded..."
            if lsmod | grep -q kvm; then
                log_warn "KVM kernel module is loaded but /dev/kvm is missing. Check dmesg or virtualization settings in your BIOS/hypervisor."
            else
                log_error "KVM hardware virtualization is not available or kernel module is not loaded."
                log_info "Try loading KVM module: sudo modprobe kvm && (sudo modprobe kvm_intel || sudo modprobe kvm_amd)"
                log_info "If running inside a cloud VM, ensure Nested Virtualization is enabled."
            fi
        fi

        # Package manager dependency checks
        if command -v dnf &>/dev/null; then
            log_info "Detected Fedora / RHEL / CentOS environment."
            if ! dnf list installed libkrun &>/dev/null; then
                log_info "Installing libkrun and libkrunfw via Fedora Copr..."
                sudo dnf install -y dnf-plugins-core gcc make curl pkgconfig
                sudo dnf copr enable -y slp/krun
                sudo dnf install -y libkrun libkrunfw libkrun-devel
            else
                log_success "libkrun is already installed via dnf."
            fi
        elif command -v apt-get &>/dev/null; then
            log_info "Detected Debian / Ubuntu environment."
            sudo apt-get update -qq
            sudo apt-get install -y -qq build-essential pkg-config libssl-dev curl
            if [ ! -f "/usr/lib/libkrun.so" ] && [ ! -f "/usr/local/lib/libkrun.so" ] && [ ! -f "/usr/lib/$(uname -m)-linux-gnu/libkrun.so" ]; then
                log_warn "libkrun shared library not found in default paths."
                log_info "Please ensure libkrun and libkrunfw are installed (e.g. from GitHub https://github.com/containers/libkrun)."
            else
                log_success "libkrun shared library detected."
            fi
        elif command -v pacman &>/dev/null; then
            log_info "Detected Arch Linux environment."
            sudo pacman -S --needed --noconfirm base-devel curl git
            if ! pacman -Qi libkrun &>/dev/null; then
                log_info "Install libkrun and libkrunfw from AUR: yay -S libkrun libkrunfw"
            fi
        fi

    else
        log_error "Unsupported operating system: $OS. libkrun requires Linux (KVM) or macOS (Hypervisor.framework)."
        exit 1
    fi

    # --------------------------------------------------------------------------
    # Rust Toolchain
    # --------------------------------------------------------------------------
    log_info "Checking Rust toolchain..."
    if ! command -v cargo &>/dev/null || ! command -v rustc &>/dev/null; then
        log_info "Rust toolchain not found. Installing via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
        log_success "Rust toolchain installed: $(rustc --version)"
    else
        log_success "Rust toolchain detected: $(cargo --version) | $(rustc --version)"
    fi
else
    log_info "Skipping dependency checks (--skip-deps specified)."
fi

# ==============================================================================
# 2. CARGO BUILD
# ==============================================================================
if [ "$DO_BUILD" = true ]; then
    log_step "Building Workspace Binaries ($BUILD_MODE mode)"
    
    BUILD_FLAGS=("--workspace")
    if [ "$BUILD_MODE" = "release" ]; then
        BUILD_FLAGS+=("--release")
    fi

    log_info "Running: cargo build ${BUILD_FLAGS[*]}"
    cargo build "${BUILD_FLAGS[@]}"
    log_success "Workspace compiled successfully."
else
    log_info "Skipping cargo build (--no-build specified)."
fi

TARGET_SUBDIR="target/$BUILD_MODE"
BIN_CLI="$TARGET_SUBDIR/microvm"
BIN_RUNNER="$TARGET_SUBDIR/microvm-runner"
BIN_SHIM="$TARGET_SUBDIR/containerd-shim-krun-v2"
BIN_OPERATOR="$TARGET_SUBDIR/krun-operator"

for b in "$BIN_CLI" "$BIN_RUNNER" "$BIN_SHIM" "$BIN_OPERATOR"; do
    if [ ! -f "$b" ]; then
        log_error "Binary not found: $b. Did compilation fail?"
        exit 1
    fi
done

# ==============================================================================
# 3. MACOS CODE SIGNING (HYPERVISOR ENTITLEMENT)
# ==============================================================================
if [ "$OS" = "Darwin" ] && [ "$DO_SIGN" = true ]; then
    log_step "Applying macOS Hypervisor.framework Code Signing Entitlements"
    
    ENTITLEMENTS_FILE="$WORKSPACE_DIR/entitlements.plist"
    if [ -f "$ENTITLEMENTS_FILE" ]; then
        log_info "Signing binaries with $ENTITLEMENTS_FILE..."
        codesign --entitlements "$ENTITLEMENTS_FILE" --force -s - "$BIN_RUNNER"
        codesign --entitlements "$ENTITLEMENTS_FILE" --force -s - "$BIN_CLI"
        codesign --entitlements "$ENTITLEMENTS_FILE" --force -s - "$BIN_SHIM"
        log_success "Binaries signed with com.apple.security.hypervisor entitlement."
    else
        log_warn "entitlements.plist not found at $ENTITLEMENTS_FILE; skipping codesign."
    fi
fi

# ==============================================================================
# 4. INSTALL BINARIES TO PREFIX
# ==============================================================================
log_step "Installing Binaries to $PREFIX"

# Create destination if needed
NEED_SUDO=false
if [ ! -d "$PREFIX" ]; then
    if [ -w "$(dirname "$PREFIX")" ]; then
        mkdir -p "$PREFIX"
    else
        NEED_SUDO=true
        sudo mkdir -p "$PREFIX"
    fi
elif [ ! -w "$PREFIX" ]; then
    NEED_SUDO=true
fi

SUDO_CMD=""
if [ "$NEED_SUDO" = true ]; then
    log_info "Target directory $PREFIX requires root privileges."
    SUDO_CMD="sudo"
fi

install_bin() {
    local src="$1"
    local name="$2"
    log_info "Installing $name -> $PREFIX/$name"
    $SUDO_CMD install -m 755 "$src" "$PREFIX/$name"
}

install_bin "$BIN_CLI" "microvm"
install_bin "$BIN_RUNNER" "microvm-runner"
install_bin "$BIN_SHIM" "containerd-shim-krun-v2"
install_bin "$BIN_OPERATOR" "krun-operator"

log_success "All binaries successfully installed to $PREFIX."

# ==============================================================================
# 5. POST-INSTALL VERIFICATION & SYSTEM STATUS
# ==============================================================================
log_step "Running Preflight Verification"

# Verify PATH
if [[ ":$PATH:" != *":$PREFIX:"* ]]; then
    log_warn "$PREFIX is not in your current PATH."
    log_info "Add it to your shell configuration (.bashrc, .zshrc):"
    printf "    export PATH=\"%s:\$PATH\"\n\n" "$PREFIX"
fi

# Run preflight checks via the installed microvm binary
if command -v "$PREFIX/microvm" &>/dev/null; then
    "$PREFIX/microvm" preflight || true
    printf "\n"
    "$PREFIX/microvm" info || true
fi

printf "\n${GREEN}${BOLD}======================================================${NC}\n"
printf "${GREEN}${BOLD}     libkrun-microvm Installed Successfully!         ${NC}\n"
printf "${GREEN}${BOLD}======================================================${NC}\n"
printf "%b" "
${BOLD}Quickstart Commands:${NC}
  Run an OCI image in a microVM:
    ${CYAN}microvm run alpine:latest -- echo \"Hello from isolated microVM!\"${NC}

  View running microVMs:
    ${CYAN}microvm ps${NC}

  Stream real-time CPU & memory telemetry:
    ${CYAN}microvm stats${NC}

  Inspect system & cache status:
    ${CYAN}microvm info${NC}

  Configure containerd for Kubernetes:
    See ${CYAN}k8s/README.md${NC} for RuntimeClass and containerd config.

"
