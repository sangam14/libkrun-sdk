use std::path::{Path, PathBuf};

enum LibKrunVariant {
    Krun(Option<PathBuf>),
    KrunEfi(Option<PathBuf>),
    PkgConfig,
    None,
}

fn detect_libkrun() -> LibKrunVariant {
    // 1. Explicit environment override
    if let Ok(dir) = std::env::var("LIBKRUN_DIR") {
        let p = PathBuf::from(&dir);
        if p.join("libkrun.dylib").exists() || p.join("libkrun.so").exists() {
            return LibKrunVariant::Krun(Some(p));
        }
        if p.join("libkrun-efi.dylib").exists() || p.join("libkrun-efi.so").exists() {
            return LibKrunVariant::KrunEfi(Some(p));
        }
    }

    // 2. Local submodule build in workspace (e.g. libkrun/target/release)
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let submodule_candidates = [
            PathBuf::from(&manifest_dir).join("../../libkrun/target/release"),
            PathBuf::from(&manifest_dir).join("../../libkrun/target/debug"),
        ];
        for dir in &submodule_candidates {
            if dir.join("libkrun.dylib").exists() || dir.join("libkrun.so").exists() {
                if let Ok(canonical) = dir.canonicalize() {
                    return LibKrunVariant::Krun(Some(canonical));
                }
            }
        }
    }

    // 3. pkg-config detection (supports /usr/local/lib/pkgconfig, Homebrew, distribution paths)
    if pkg_config::Config::new().probe("libkrun").is_ok() {
        return LibKrunVariant::PkgConfig;
    }
    if pkg_config::Config::new().probe("libkrun-efi").is_ok() {
        return LibKrunVariant::PkgConfig;
    }

    // 4. Standard filesystem candidate search paths
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
        let p = Path::new(path);
        if p.exists() {
            let parent = p.parent().map(|p| p.to_path_buf());
            return LibKrunVariant::Krun(parent);
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
        let p = Path::new(path);
        if p.exists() {
            let parent = p.parent().map(|p| p.to_path_buf());
            return LibKrunVariant::KrunEfi(parent);
        }
    }

    LibKrunVariant::None
}

fn main() {
    println!("cargo:rerun-if-changed=stub.c");
    println!("cargo:rerun-if-env-changed=LIBKRUN_DIR");
    println!("cargo:rerun-if-env-changed=FORCE_LIBKRUN_STUB");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");

    let force_stub = std::env::var("FORCE_LIBKRUN_STUB").is_ok();
    let variant = if force_stub {
        LibKrunVariant::None
    } else {
        detect_libkrun()
    };

    match variant {
        LibKrunVariant::Krun(custom_dir) => {
            println!("cargo:rustc-link-lib=dylib=krun");
            add_search_paths(custom_dir.as_deref());
        }
        LibKrunVariant::KrunEfi(custom_dir) => {
            println!("cargo:rustc-link-lib=dylib=krun-efi");
            add_search_paths(custom_dir.as_deref());
        }
        LibKrunVariant::PkgConfig => {
            // pkg_config has already emitted link-lib and search paths.
            // Add standard rpaths to ensure runtime dlopen succeeds.
            add_standard_rpaths();
        }
        LibKrunVariant::None => {
            println!("cargo:warning=libkrun native library not found; compiling fallback stub for test/CI compatibility");
            cc::Build::new().file("stub.c").compile("krun");
        }
    }
}

fn add_search_paths(custom_dir: Option<&Path>) {
    if let Some(dir) = custom_dir {
        let dir_str = dir.display().to_string();
        println!("cargo:rustc-link-search=native={dir_str}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir_str}");
    }

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-search=native=/opt/homebrew/lib");
        println!("cargo:rustc-link-search=native=/usr/local/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");
    }

    #[cfg(target_os = "linux")]
    {
        let linux_paths = [
            "/usr/local/lib",
            "/usr/lib",
            "/usr/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
        ];
        for p in &linux_paths {
            if Path::new(p).exists() {
                println!("cargo:rustc-link-search=native={p}");
                println!("cargo:rustc-link-arg=-Wl,-rpath,{p}");
            }
        }
    }
}

fn add_standard_rpaths() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/opt/homebrew/lib");
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/local/lib");
    }

    #[cfg(target_os = "linux")]
    {
        let linux_paths = [
            "/usr/local/lib",
            "/usr/lib",
            "/usr/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
        ];
        for p in &linux_paths {
            if Path::new(p).exists() {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{p}");
            }
        }
    }
}
