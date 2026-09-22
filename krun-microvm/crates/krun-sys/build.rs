use std::path::Path;

enum LibKrunVariant {
    Krun,
    KrunEfi,
    None,
}

fn detect_libkrun() -> LibKrunVariant {
    let krun_candidates = [
        "/opt/homebrew/lib/libkrun.dylib",
        "/usr/local/lib/libkrun.dylib",
        "/usr/lib/libkrun.so",
        "/usr/lib64/libkrun.so",
        "/usr/local/lib/libkrun.so",
        "/usr/lib/x86_64-linux-gnu/libkrun.so",
        "/usr/lib/aarch64-linux-gnu/libkrun.so",
    ];
    for path in &krun_candidates {
        if Path::new(path).exists() {
            return LibKrunVariant::Krun;
        }
    }
    if let Ok(dir) = std::env::var("LIBKRUN_DIR") {
        if Path::new(&dir).join("libkrun.dylib").exists()
            || Path::new(&dir).join("libkrun.so").exists()
        {
            return LibKrunVariant::Krun;
        }
        if Path::new(&dir).join("libkrun-efi.dylib").exists()
            || Path::new(&dir).join("libkrun-efi.so").exists()
        {
            return LibKrunVariant::KrunEfi;
        }
    }
    let efi_candidates = [
        "/opt/homebrew/lib/libkrun-efi.dylib",
        "/usr/local/lib/libkrun-efi.dylib",
        "/usr/lib/libkrun-efi.so",
        "/usr/lib64/libkrun-efi.so",
        "/usr/local/lib/libkrun-efi.so",
    ];
    for path in &efi_candidates {
        if Path::new(path).exists() {
            return LibKrunVariant::KrunEfi;
        }
    }
    LibKrunVariant::None
}

fn main() {
    println!("cargo:rerun-if-changed=stub.c");
    println!("cargo:rerun-if-env-changed=LIBKRUN_DIR");
    println!("cargo:rerun-if-env-changed=FORCE_LIBKRUN_STUB");

    let force_stub = std::env::var("FORCE_LIBKRUN_STUB").is_ok();
    let variant = if force_stub {
        LibKrunVariant::None
    } else {
        detect_libkrun()
    };

    match variant {
        LibKrunVariant::Krun => {
            println!("cargo:rustc-link-lib=dylib=krun");
            add_search_paths();
        }
        LibKrunVariant::KrunEfi => {
            println!("cargo:rustc-link-lib=dylib=krun-efi");
            add_search_paths();
        }
        LibKrunVariant::None => {
            println!("cargo:warning=libkrun native library not found; compiling fallback stub for test/CI compatibility");
            cc::Build::new().file("stub.c").compile("krun");
        }
    }
}

fn add_search_paths() {
    if let Ok(dir) = std::env::var("LIBKRUN_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        #[cfg(target_os = "macos")]
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
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
}
