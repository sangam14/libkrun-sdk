//! Example: Hardware-Isolated LLM Inference with mistral.rs and krun-microvm (Pattern 1)
//!
//! Demonstrates running an OpenAI-compatible mistral.rs inference server inside a
//! hardware-isolated microVM, attaching models via VirtioFS / OCI artifacts, and
//! querying the completion endpoint over TSI port forwarding.

use anyhow::Result;
use microvm_core::MicroVmBuilder;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!("============================================================");
    println!(" 🤖 krun-microvm: mistral.rs Hardware-Isolated Inference");
    println!("============================================================");

    // 1. Configure paths and port
    let host_port = 1234;
    let guest_port = 1234;
    let models_dir = std::env::var("MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./models"));

    // Check if a local GGUF model exists, otherwise demonstrate Hugging Face mode
    let gguf_model = std::env::var("GGUF_FILE")
        .unwrap_or_else(|_| "mistral-7b-instruct-v0.2.Q4_K_M.gguf".to_string());
    let hf_model = std::env::var("HF_MODEL")
        .unwrap_or_else(|_| "mistralai/Mistral-7B-Instruct-v0.2".to_string());
    let use_gguf = models_dir.join(&gguf_model).exists();

    println!("⚙️  Inference Configuration:");
    println!("   - Host Port Forward: 127.0.0.1:{host_port} -> guest:{guest_port}");
    println!("   - Memory Allocation: 4096 MiB (4 GiB)");
    println!("   - vCPUs: 4");
    if use_gguf {
        println!(
            "   - Engine Mode: Local GGUF ({})",
            models_dir.join(&gguf_model).display()
        );
    } else {
        println!(
            "   - Engine Mode: Hugging Face with In-Situ Quantization ({})",
            hf_model
        );
    }

    // 2. Build mistral.rs execution command
    // Note: --host 0.0.0.0 is critical so TSI can proxy requests into the microVM
    let cmd = if use_gguf {
        vec![
            "mistralrs-server".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            guest_port.to_string(),
            "gguf".to_string(),
            "-m".to_string(),
            "/models".to_string(),
            "-f".to_string(),
            gguf_model,
        ]
    } else {
        vec![
            "mistralrs-server".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            guest_port.to_string(),
            "plain".to_string(),
            "-m".to_string(),
            hf_model,
            "--isq".to_string(),
            "Q4K".to_string(),
        ]
    };

    // 3. Construct the MicroVm
    let mut builder = MicroVmBuilder::new("ghcr.io/ericlbuehler/mistral.rs:cpu-latest")
        .cpus(4)
        .memory_mb(4096)
        .port_forward(host_port, guest_port)
        .cmd(cmd);

    // Forward HF_TOKEN if available in the host environment
    if let Ok(token) = std::env::var("HF_TOKEN") {
        builder = builder.env("HF_TOKEN", &token);
    }

    // Mount local models directory via VirtioFS if it exists
    if models_dir.exists() {
        println!("📂 Mounting local models directory at /models via VirtioFS...");
        builder = builder.virtiofs("models", &models_dir, true);
    }

    println!("🚀 Launching hardware-isolated mistral.rs microVM...");
    let mut vm = builder.run().await?;
    println!("✅ MicroVM active! ID: {}, PID: {:?}", vm.id(), vm.pid());

    // 4. Poll health endpoint until mistralrs-server is ready
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let health_url = format!("http://127.0.0.1:{host_port}/health");
    let completions_url = format!("http://127.0.0.1:{host_port}/v1/chat/completions");

    println!("⏳ Waiting for mistralrs-server to initialize (polling {health_url})...");
    let start = std::time::Instant::now();
    let max_wait = Duration::from_secs(60);
    let mut ready = false;

    while start.elapsed() < max_wait {
        if !vm.is_alive() {
            println!("⚠️  MicroVM terminated unexpectedly before serving.");
            break;
        }

        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                ready = true;
                println!(
                    "🎉 mistralrs-server is ready and healthy! (in {:.1}s)",
                    start.elapsed().as_secs_f64()
                );
                break;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }

    // 5. Send a test chat completion if server became ready
    if ready {
        println!("\n💬 Sending test OpenAI-compatible chat completion prompt...");
        let payload = serde_json::json!({
            "model": "default",
            "messages": [
                {"role": "system", "content": "You are a helpful and concise assistant inside a secure microVM."},
                {"role": "user", "content": "Explain what a microVM is in one sentence."}
            ],
            "max_tokens": 64,
            "temperature": 0.7
        });

        match client.post(&completions_url).json(&payload).send().await {
            Ok(resp) => {
                let status = resp.status();
                let body: serde_json::Value = resp.json().await.unwrap_or_default();
                println!("Response Status: {status}");
                if let Some(reply) = body["choices"][0]["message"]["content"].as_str() {
                    println!("\n🧠 Model Output:\n{reply}\n");
                } else {
                    println!("Response Body:\n{:#}", body);
                }
            }
            Err(e) => {
                println!("Failed to query completion API: {e}");
            }
        }
    } else {
        println!("ℹ️  Tip: If pulling large weights from Hugging Face for the first time, set HF_TOKEN and allow time for weights download.");
    }

    // 6. Graceful termination
    println!("🛑 Stopping mistral.rs microVM...");
    vm.stop().await?;
    println!("✅ MicroVM cleanly stopped. All host resources reclaimed.");

    Ok(())
}
