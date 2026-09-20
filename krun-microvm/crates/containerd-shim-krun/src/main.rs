use containerd_shim::asynchronous::run;

mod metrics;
mod service;
use service::KrunShim;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "-v" || args[1] == "--version" || args[1] == "-version") {
        println!("containerd-shim-krun-v2 (krun-microvm) version 0.1.0");
        return Ok(());
    }
    if args.len() > 1 && (args[1] == "-h" || args[1] == "--help") {
        println!("containerd-shim-krun-v2: containerd v2 shim for krun-microvm (hardware-isolated microVMs)");
        println!("Usage: containerd-shim-krun-v2 [flags] [start|delete]");
        println!("This binary is managed automatically by containerd via RuntimeClass 'krun'.");
        return Ok(());
    }
    run::<KrunShim>("io.containerd.krun.v2", None).await;
    Ok(())
}
