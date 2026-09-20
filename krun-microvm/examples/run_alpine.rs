use anyhow::Result;
use microvm_core::MicroVmBuilder;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!("Starting Alpine Linux microVM using Rust SDK...");
    let mut vm = MicroVmBuilder::new("alpine:latest")
        .cpus(2)
        .memory_mb(512)
        .cmd(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo 'Hello from inside a hardware-isolated microVM!' && uname -a && cat /etc/os-release".to_string(),
        ])
        .run()
        .await?;

    println!("MicroVM running with PID {:?}", vm.pid());
    let status = vm.wait().await?;
    println!("MicroVM finished with status: {status}");
    Ok(())
}
