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
            tracing::info!(
                "Cleaning up backing pod for MicroVm '{}/{}'...",
                namespace,
                name
            );
            let pod_name = format!("microvm-{}", name);
            let _ = pods.delete(&pod_name, &Default::default()).await;

            // Remove finalizer
            let patch = serde_json::json!({
                "metadata": {
                    "finalizers": vm.finalizers().iter().filter(|f| *f != FINALIZER_NAME).collect::<Vec<_>>()
                }
            });
            vms.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
                .await?;
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
        vms.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?;
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

            // Format container ports
            let mut container_ports = Vec::new();
            let mut port_forward_strings = Vec::new();
            if let Some(ref ports) = vm.spec.ports {
                for p in ports {
                    let mut port_obj = serde_json::json!({
                        "containerPort": p.container_port,
                    });
                    if let Some(hp) = p.host_port {
                        port_obj["hostPort"] = serde_json::json!(hp);
                        port_forward_strings.push(format!("{}:{}", hp, p.container_port));
                    } else {
                        port_forward_strings.push(format!("{}:{}", p.container_port, p.container_port));
                    }
                    if let Some(ref proto) = p.protocol {
                        port_obj["protocol"] = serde_json::json!(proto);
                    }
                    if let Some(ref name) = p.name {
                        port_obj["name"] = serde_json::json!(name);
                    }
                    container_ports.push(port_obj);
                }
            } else if let Some(p) = vm.spec.port {
                container_ports.push(serde_json::json!({
                    "containerPort": p,
                    "name": "vm-port"
                }));
                port_forward_strings.push(format!("{}:{}", p, p));
            }

            let ports_json = if container_ports.is_empty() {
                None
            } else {
                Some(container_ports)
            };

            // Format volume mounts & host volumes
            let mut pod_volumes = Vec::new();
            let mut container_volume_mounts = Vec::new();
            if let Some(ref mounts) = vm.spec.volume_mounts {
                for m in mounts {
                    pod_volumes.push(serde_json::json!({
                        "name": m.name,
                        "hostPath": {
                            "path": m.host_path,
                            "type": "DirectoryOrCreate"
                        }
                    }));
                    container_volume_mounts.push(serde_json::json!({
                        "name": m.name,
                        "mountPath": m.mount_path,
                        "readOnly": m.read_only.unwrap_or(false)
                    }));
                }
            }

            // Construct annotations
            let mut annotations = serde_json::Map::new();

            // DAX
            if let Some(ref dax) = vm.spec.dax_window_size {
                annotations.insert(
                    "krun.dax".to_string(),
                    serde_json::Value::String(dax.clone()),
                );
                annotations.insert(
                    "krun.io/dax-window-size".to_string(),
                    serde_json::Value::String(dax.clone()),
                );
            }

            // GPU
            if let Some(true) = vm.spec.gpu {
                annotations.insert(
                    "krun.gpu".to_string(),
                    serde_json::Value::String("true".to_string()),
                );
                annotations.insert(
                    "krun.io/gpu".to_string(),
                    serde_json::Value::String("true".to_string()),
                );
            }
            if let Some(ref shm) = vm.spec.gpu_shm_size {
                annotations.insert(
                    "krun.gpu.shm".to_string(),
                    serde_json::Value::String(shm.clone()),
                );
                annotations.insert(
                    "krun.io/gpu-shm-size".to_string(),
                    serde_json::Value::String(shm.clone()),
                );
            }

            // Sandbox
            if let Some(sb) = vm.spec.sandbox {
                annotations.insert(
                    "krun.sandbox".to_string(),
                    serde_json::Value::String(sb.to_string()),
                );
                annotations.insert(
                    "krun.io/sandbox".to_string(),
                    serde_json::Value::String(sb.to_string()),
                );
            }

            // Network Mode
            if let Some(ref net) = vm.spec.network_mode {
                annotations.insert(
                    "krun.network".to_string(),
                    serde_json::Value::String(net.clone()),
                );
                annotations.insert(
                    "krun.io/network-mode".to_string(),
                    serde_json::Value::String(net.clone()),
                );
            }

            // Egress allowlist
            if let Some(ref egress) = vm.spec.allow_egress {
                let joined = egress.join(",");
                annotations.insert(
                    "krun.network.allow".to_string(),
                    serde_json::Value::String(joined.clone()),
                );
                annotations.insert(
                    "krun.io/allow-egress".to_string(),
                    serde_json::Value::String(joined),
                );
            }

            // Token budget
            if let Some(budget) = vm.spec.token_budget {
                annotations.insert(
                    "krun.token-budget".to_string(),
                    serde_json::Value::String(budget.to_string()),
                );
                annotations.insert(
                    "krun.io/token-budget".to_string(),
                    serde_json::Value::String(budget.to_string()),
                );
            }

            // DNS
            if let Some(ref dns) = vm.spec.dns_servers {
                let joined = dns.join(",");
                annotations.insert(
                    "krun.dns".to_string(),
                    serde_json::Value::String(joined.clone()),
                );
                annotations.insert(
                    "krun.io/dns-servers".to_string(),
                    serde_json::Value::String(joined),
                );
            }

            // Ports annotations
            if !port_forward_strings.is_empty() {
                let joined = port_forward_strings.join(",");
                annotations.insert(
                    "krun.ports".to_string(),
                    serde_json::Value::String(joined.clone()),
                );
                annotations.insert(
                    "krun.io/ports".to_string(),
                    serde_json::Value::String(joined),
                );
            }

            // Model artifact
            if let Some(ref model) = vm.spec.model_artifact {
                annotations.insert(
                    "krun.model-artifact".to_string(),
                    serde_json::Value::String(model.clone()),
                );
                annotations.insert(
                    "krun.io/model-artifact".to_string(),
                    serde_json::Value::String(model.clone()),
                );
            }

            // Workspace CoW
            if let Some(ref cow) = vm.spec.workspace_cow {
                annotations.insert(
                    "krun.workspace-cow".to_string(),
                    serde_json::Value::String(cow.clone()),
                );
                annotations.insert(
                    "krun.io/workspace-cow".to_string(),
                    serde_json::Value::String(cow.clone()),
                );
            }

            // Image acceleration
            if let Some(ref acc) = vm.spec.image_acceleration {
                if let Some(ref fmt) = acc.format {
                    annotations.insert(
                        "krun.io/image-acceleration-format".to_string(),
                        serde_json::Value::String(fmt.clone()),
                    );
                }
                if let Some(lazy) = acc.lazy_load {
                    annotations.insert(
                        "krun.io/image-acceleration-lazy-load".to_string(),
                        serde_json::Value::String(lazy.to_string()),
                    );
                }
                if let Some(ref sz) = acc.chunk_size {
                    annotations.insert(
                        "krun.io/image-acceleration-chunk-size".to_string(),
                        serde_json::Value::String(sz.clone()),
                    );
                }
            }

            // Propagate custom pod annotations
            if let Some(ref custom_ann) = vm.spec.pod_annotations {
                for (k, v) in custom_ann {
                    annotations.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }

            // Build labels
            let mut labels = serde_json::Map::new();
            labels.insert(
                "app.kubernetes.io/name".to_string(),
                serde_json::Value::String("krun-microvm".to_string()),
            );
            labels.insert(
                "krun.io/microvm".to_string(),
                serde_json::Value::String(name.clone()),
            );
            if let Some(ref custom_lbls) = vm.spec.pod_labels {
                for (k, v) in custom_lbls {
                    labels.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
            }

            let mut spec_obj = serde_json::json!({
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
            });
            if !container_volume_mounts.is_empty() {
                spec_obj["containers"][0]["volumeMounts"] = serde_json::json!(container_volume_mounts);
                spec_obj["volumes"] = serde_json::json!(pod_volumes);
            }

            let pod_manifest: Pod = serde_json::from_value(serde_json::json!({
                "apiVersion": "v1",
                "kind": "Pod",
                "metadata": {
                    "name": pod_name,
                    "namespace": namespace,
                    "labels": labels,
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
                "spec": spec_obj
            }))?;

            pods.create(&PostParams::default(), &pod_manifest).await?;

            // Update initial status
            let initial_conditions = vec![
                crate::crd::MicroVmCondition {
                    type_: "PodScheduled".to_string(),
                    status: "True".to_string(),
                    last_transition_time: None,
                    reason: Some("PodCreated".to_string()),
                    message: Some("Backing Pod created by krun-operator".to_string()),
                },
                crate::crd::MicroVmCondition {
                    type_: "Ready".to_string(),
                    status: "False".to_string(),
                    last_transition_time: None,
                    reason: Some("ContainersNotReady".to_string()),
                    message: Some("MicroVM boot in progress".to_string()),
                },
            ];

            let new_status = MicroVmStatus {
                phase: "Pending".to_string(),
                pod_name: Some(pod_name),
                pod_ip: None,
                node_name: None,
                ready: false,
                message: Some("Backing Pod scheduled with runtimeClassName: krun".to_string()),
                conditions: Some(initial_conditions),
            };

            let patch = serde_json::json!({ "status": new_status });
            vms.patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch))
                .await?;

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

            let conditions = pod_status.and_then(|s| s.conditions.as_ref()).map(|cs| {
                cs.iter()
                    .map(|c| crate::crd::MicroVmCondition {
                        type_: c.type_.clone(),
                        status: c.status.clone(),
                        last_transition_time: c.last_transition_time.as_ref().map(|t| t.0.to_string()),
                        reason: c.reason.clone(),
                        message: c.message.clone(),
                    })
                    .collect::<Vec<_>>()
            });

            let new_status = MicroVmStatus {
                phase,
                pod_name: Some(pod_name),
                pod_ip,
                node_name,
                ready,
                message,
                conditions,
            };

            let patch = serde_json::json!({ "status": new_status });
            vms.patch_status(&name, &PatchParams::default(), &Patch::Merge(&patch))
                .await?;

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
                    tracing::debug!(
                        "Reconciled MicroVm: {}/{}",
                        o.namespace.unwrap_or_default(),
                        o.name
                    );
                }
                Err(e) => {
                    tracing::warn!("Reconcile error: {:?}", e);
                }
            }
        })
        .await;

    Ok(())
}
