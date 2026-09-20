fn main() {
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
    }
}
