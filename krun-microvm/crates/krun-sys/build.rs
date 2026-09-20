use std::path::Path;

fn has_libkrun() -> bool {
    let candidates = [
        "/opt/homebrew/lib/libkrun.dylib",
        "/usr/local/lib/libkrun.dylib",
        "/usr/lib/libkrun.so",
        "/usr/lib64/libkrun.so",
        "/usr/local/lib/libkrun.so",
        "/usr/lib/x86_64-linux-gnu/libkrun.so",
        "/usr/lib/aarch64-linux-gnu/libkrun.so",
    ];
    for path in &candidates {
        if Path::new(path).exists() {
            return true;
        }
    }
    if let Ok(dir) = std::env::var("LIBKRUN_DIR") {
        if Path::new(&dir).join("libkrun.dylib").exists()
            || Path::new(&dir).join("libkrun.so").exists()
        {
            return true;
        }
    }
    false
}

fn main() {
    println!("cargo:rerun-if-changed=stub.c");
    println!("cargo:rerun-if-env-changed=LIBKRUN_DIR");
    println!("cargo:rerun-if-env-changed=FORCE_LIBKRUN_STUB");

    let force_stub = std::env::var("FORCE_LIBKRUN_STUB").is_ok();

    if !force_stub && has_libkrun() {
        println!("cargo:rustc-link-lib=dylib=krun");
        #[cfg(target_os = "macos")]
        {
            println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
            println!("cargo:rustc-link-search=native=/usr/local/lib");
        }
        #[cfg(target_os = "linux")]
        {
            println!("cargo:rustc-link-search=native=/usr/local/lib");
            println!("cargo:rustc-link-search=native=/usr/lib");
            println!("cargo:rustc-link-search=native=/usr/lib64");
            println!("cargo:rustc-link-search=native=/usr/lib/x86_64-linux-gnu");
            println!("cargo:rustc-link-search=native=/usr/lib/aarch64-linux-gnu");
        }
    } else {
        println!("cargo:warning=libkrun native library not found; compiling fallback stub for test/CI compatibility");
        cc::Build::new().file("stub.c").compile("krun");
    }
}
