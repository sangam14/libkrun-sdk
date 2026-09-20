mod controller;
mod crd;

use anyhow::Result;
use clap::Parser;
use crd::MicroVm;
use kube::CustomResourceExt;

#[derive(Parser, Debug)]
#[command(
    name = "krun-operator",
    version,
    about = "Kubernetes Operator for hardware-isolated libkrun microVMs"
)]
struct Args {
    /// Export the CustomResourceDefinition (CRD) YAML to stdout
    #[arg(long)]
    export_crd: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.export_crd {
        let crd = MicroVm::crd();
        let yaml = serde_yaml::to_string(&crd)?;
        print!("{yaml}");
        return Ok(());
    }

    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "krun_operator=info,kube=info".into()),
        )
        .init();

    tracing::info!("Initializing krun-operator with kube-rs...");
    let client = kube::Client::try_default().await?;

    controller::run_controller(client).await?;

    Ok(())
}
