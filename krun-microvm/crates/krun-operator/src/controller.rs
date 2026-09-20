use crate::crd::{MicroVm, MicroVmStatus};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::runtime::controller::{Action, Controller};
use kube::{Client, ResourceExt};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

pub const FINALIZER_NAME: &str = "krun.io/finalizer";

#[derive(Error, Debug)]
pub enum Error {
    #[error("Kube error: {0}")]
    Kube(#[from] kube::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub struct ContextData {
    pub client: Client,
}

/// Reconciler loop for MicroVm custom resources
pub async fn reconcile(vm: Arc<MicroVm>, ctx: Arc<ContextData>) -> Result<Action, Error> {
    let name = vm.name_any();
    let namespace = vm.namespace().unwrap_or_else(|| "default".to_string());
    let vms: Api<MicroVm> = Api::namespaced(ctx.client.clone(), &namespace);
    let pods: Api<Pod> = Api::namespaced(ctx.client.clone(), &namespace);

    // 1. Handle deletion / finalizers
    if vm.metadata.deletion_timestamp.is_some() {
        if vm.finalizers().iter().any(|f| f == FINALIZER_NAME) {
            tracing::info!("Cleaning up backing pod for MicroVm '{}/{}'...", namespace, name);
            let pod_name = format!("microvm-{}", name);
            let _ = pods.delete(&pod_name, &Default::default()).await;

            // Remove finalizer
            let patch = serde_json::json!({
                "metadata": {
                    "finalizers": vm.finalizers().iter().filter(|f| *f != FINALIZER_NAME).collect::<Vec<_>>()
                }
            });
            vms.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await?;
            tracing::info!("Removed finalizer from MicroVm '{}/{}'", namespace, name);
        }
        return Ok(Action::await_change());
    }

    // 2. Ensure finalizer is present
    if !vm.finalizers().iter().any(|f| f == FINALIZER_NAME) {
        let mut finalizers = vm.finalizers().to_vec();
        finalizers.push(FINALIZER_NAME.to_string());
        let patch = serde_json::json!({
            "metadata": {
                "finalizers": finalizers
            }
        });
        vms.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await?;
    }

    // 3. Check for backing Pod
    let pod_name = format!("microvm-{}", name);
    let existing_pod = pods.get_opt(&pod_name).await?;

    match existing_pod {
        None => {
            tracing::info!(
                "Creating hardware-isolated backing Pod '{}' for MicroVm '{}/{}'...",
                pod_name,
                namespace,
                name
            );

            // Format environment variables
            let env_json = vm.spec.env.as_ref().map(|vars| {
                vars.iter()
                    .map(|e| serde_json::json!({ "name": e.name, "value": e.value }))
                    .collect::<Vec<_>>()
            });

            // Format ports
            let ports_json = vm.spec.port.map(|p| {
                vec![serde_json::json!({
                    "containerPort": p,
                    "name": "vm-port"
                })]
            });

            let mut annotations = serde_json::Map::new();
            if let Some(ref dax) = vm.spec.dax_window_size {
                annotations.insert("krun.io/dax-window-size".to_string(), serde_json::Value::String(dax.clone()));
            }
            if let Some(ref model) = vm.spec.model_artifact {
                annotations.insert("krun.io/model-artifact".to_string(), serde_json::Value::String(model.clone()));
            }

            let pod_manifest: Pod = serde_json::from_value(serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "name": pod_name,
                    "namespace": namespace,
                    "labels": {
                        "app.kubernetes.io/name": "krun-microvm",
                        "krun.io/microvm": name,
                    },
                    "annotations": annotations,
                    "ownerReferences": [{
                        "apiVersion": "krun.io/v1alpha1",
                        "kind": "MicroVm",
                        "name": name,
                        "uid": vm.metadata.uid.as_deref().unwrap_or_default(),
                        "controller": true,
                        "blockOwnerDeletion": true,
                    }]
                },
                "spec": {
                    // Critical: Specifies containerd-shim-krun-v2 runtime handler
                    "runtimeClassName": "krun",
                    "containers": [{
                        "name": "microvm",
                        "image": vm.spec.image,
                        "command": vm.spec.cmd,
                        "env": env_json,
                        "ports": ports_json,
                        "resources": {
                            "limits": {
                                "cpu": format!("{}", vm.spec.vcpus),
                                "memory": vm.spec.memory,
                            },
                            "requests": {
                                "cpu": format!("{}", vm.spec.vcpus),
                                "memory": vm.spec.memory,
                            }
                        }
                    }]
                }
            }))?;

            pods.create(&PostParams::default(), &pod_manifest).await?;

            // Update initial status
            let new_status = MicroVmStatus {
                phase: "Pending".to_string(),
                pod_name: Some(pod_name),
                pod_ip: None,
                node_name: None,
                ready: false,
                message: Some("Backing Pod scheduled with runtimeClassName: krun".to_string()),
            };

            let patch = serde_json::json!({ "status": new_status });
            vms.patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch)).await?;

            Ok(Action::requeue(Duration::from_secs(5)))
        }
        Some(pod) => {
            // 4. Sync Pod status into MicroVmStatus
            let pod_status = pod.status.as_ref();
            let ready = pod_status
                .and_then(|s| s.container_statuses.as_ref())
                .map(|cs| cs.iter().all(|c| c.ready))
                .unwrap_or(false);

            let is_paused = vm.spec.paused.unwrap_or(false);
            let phase = if is_paused && ready {
                "Paused".to_string()
            } else {
                pod_status
                    .and_then(|s| s.phase.clone())
                    .unwrap_or_else(|| "Pending".to_string())
            };
            let pod_ip = pod_status.and_then(|s| s.pod_ip.clone());
            let node_name = pod.spec.as_ref().and_then(|s| s.node_name.clone());

            let message = if is_paused && ready {
                Some("MicroVM vCPUs are paused (declarative spec.paused: true)".to_string())
            } else if ready {
                Some("Hardware-isolated microVM is running and ready".to_string())
            } else {
                pod_status.and_then(|s| s.message.clone())
            };

            let new_status = MicroVmStatus {
                phase,
                pod_name: Some(pod_name),
                pod_ip,
                node_name,
                ready,
                message,
            };

            let patch = serde_json::json!({ "status": new_status });
            vms.patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch)).await?;

            Ok(Action::requeue(Duration::from_secs(15)))
        }
    }
}

pub fn on_error(vm: Arc<MicroVm>, error: &Error, _ctx: Arc<ContextData>) -> Action {
    tracing::error!(
        "Reconciliation error on MicroVm '{}/{}': {:?}",
        vm.namespace().unwrap_or_default(),
        vm.name_any(),
        error
    );
    Action::requeue(Duration::from_secs(5))
}

pub async fn run_controller(client: Client) -> Result<(), Error> {
    let vms: Api<MicroVm> = Api::all(client.clone());
    let pods: Api<Pod> = Api::all(client.clone());
    let context = Arc::new(ContextData { client });

    tracing::info!("Starting krun-operator Controller loop (watching MicroVms and Pods)...");

    Controller::new(vms, Default::default())
        .owns(pods, Default::default())
        .run(reconcile, on_error, context)
        .for_each(|res| async move {
            match res {
                Ok((o, _action)) => {
                    tracing::debug!("Reconciled MicroVm: {}/{}", o.namespace.unwrap_or_default(), o.name);
                }
                Err(e) => {
                    tracing::warn!("Reconcile error: {:?}", e);
                }
            }
        })
        .await;

    Ok(())
}
