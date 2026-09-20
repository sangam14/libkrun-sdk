use anyhow::Result;
use microvm_core::MicroVmBuilder;
use std::fs;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // 1. Prepare a sample host workspace
    let sample_project = std::env::temp_dir().join("sample_agent_workspace");
    fs::create_dir_all(&sample_project)?;
    fs::write(
        sample_project.join("main.py"),
        "print('Hello from original host workspace!')\n",
    )?;

    println!(
        "🧪 Host workspace prepared at: {}",
        sample_project.display()
    );
    println!("🚀 Launching MicroVM with APFS/FICLONE CoW sandboxing and artifact mounting...");

    // 2. Launch microVM with workspace CoW snapshot and artifact mounting
    let mut vm = MicroVmBuilder::new("alpine:latest")
        .cpus(2)
        .memory_mb(512)
        // Mount an isolated CoW copy of host project into /workspace
        .workspace_cow(&sample_project, "workspace")
        // Can attach arbitrary OCI artifacts (weights, datasets) via .attach_artifact()
        // .attach_artifact("ghcr.io/mistralai/mistral-7b:v0.3", "models", true)
        .cmd(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "echo '--- Guest Sandboxed Execution ---' && \
             ls -la /workspace && \
             cat /workspace/main.py && \
             echo '# Host file modified inside sandbox!' >> /workspace/main.py && \
             echo 'Modified guest workspace content:' && \
             cat /workspace/main.py"
                .to_string(),
        ])
        .run()
        .await?;

    println!("MicroVM running with PID {:?}", vm.pid());
    let status = vm.wait().await?;
    println!("MicroVM finished with status: {status}");

    // 3. Verify host file was untouched thanks to CoW isolation
    let host_content = fs::read_to_string(sample_project.join("main.py"))?;
    println!("\n🔍 Verifying host workspace integrity:");
    println!(
        "Host content after guest mutation:\n{}",
        host_content.trim()
    );
    assert!(
        !host_content.contains("Host file modified inside sandbox!"),
        "Host workspace should remain completely unaltered!"
    );
    println!("✅ SUCCESS: Host directory was 100% protected by CoW sandbox!");

    // Clean up
    let _ = fs::remove_dir_all(sample_project);
    Ok(())
}
