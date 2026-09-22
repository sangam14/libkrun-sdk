use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"));
    let bridge_dir = manifest_dir.join("gvproxy-bridge");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let lib_path = out_dir.join("libgvproxy.a");

    println!("cargo:rerun-if-changed=gvproxy-bridge/main.go");
    println!("cargo:rerun-if-changed=gvproxy-bridge/go.mod");
    println!("cargo:rerun-if-changed=gvproxy-bridge/go.sum");

    // Check that go is installed
    let go_check = Command::new("go")
        .arg("version")
        .status()
        .expect("Go compiler ('go') not found in PATH. Please install Go to build libgvproxy-sys.");
    if !go_check.success() {
        panic!("'go version' returned non-zero exit code.");
    }

    // Build the CGO static archive
    let mut cmd = Command::new("go");
    cmd.args([
        "build",
        "-buildmode=c-archive",
        "-o",
        lib_path.to_str().unwrap(),
        ".",
    ])
    .current_dir(&bridge_dir);

    let status = cmd.status().expect("Failed to execute go build command");
    if !status.success() {
        panic!("Failed to compile libgvproxy C-archive from Go sources");
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=gvproxy");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=resolv");
    } else if target_os == "linux" {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=dl");
    }
}
