use async_trait::async_trait;
use containerd_shim::asynchronous::{ExitSignal, Shim};
use containerd_shim::publisher::RemotePublisher;
use containerd_shim::{Config, Error, Flags, StartOpts, TtrpcContext};
use containerd_shim_protos::api::*;
use containerd_shim_protos::protobuf::well_known_types::timestamp::Timestamp;
use containerd_shim_protos::shim::shim_ttrpc_async::Task;
use containerd_shim_protos::ttrpc;
use containerd_shim_protos::types::task::ProcessInfo;
use microvm_core::MicroVmBuilder;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct KrunShim {
    exit: Arc<ExitSignal>,
}

#[async_trait]
impl Shim for KrunShim {
    type T = KrunTask;

    async fn new(_runtime_id: &str, _args: &Flags, _config: &mut Config) -> Self {
        Self {
            exit: Arc::new(ExitSignal::default()),
        }
    }

    async fn start_shim(&mut self, opts: StartOpts) -> Result<String, Error> {
        let grouping = opts.id.clone();
        let address = containerd_shim::spawn(opts, &grouping, vec![]).await?;
        Ok(address)
    }

    async fn delete_shim(&mut self) -> Result<DeleteResponse, Error> {
        Ok(DeleteResponse::new())
    }

    async fn wait(&mut self) {
        self.exit.wait().await;
    }

    async fn create_task_service(&self, _publisher: RemotePublisher) -> Self::T {
        KrunTask::new(self.exit.clone())
    }
}

struct TaskInstance {
    id: String,
    pid: u32,
    bundle: PathBuf,
    stdin: String,
    stdout: String,
    stderr: String,
    terminal: bool,
    status: Status,
    exit_status: Option<u32>,
    exited_at: Option<Timestamp>,
    vm: Option<Arc<Mutex<microvm_core::MicroVm>>>,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
}

struct ExecInstance {
    task_id: String,
    exec_id: String,
    stdin: String,
    stdout: String,
    stderr: String,
    terminal: bool,
    cmd: Vec<String>,
    env: Vec<String>,
    workdir: Option<String>,
    status: Status,
    exit_status: Option<u32>,
    exited_at: Option<Timestamp>,
}

#[derive(Clone)]
pub struct KrunTask {
    exit: Arc<ExitSignal>,
    instances: Arc<Mutex<HashMap<String, TaskInstance>>>,
    exec_instances: Arc<Mutex<HashMap<String, ExecInstance>>>,
}

impl KrunTask {
    pub fn new(exit: Arc<ExitSignal>) -> Self {
        Self {
            exit,
            instances: Arc::new(Mutex::new(HashMap::new())),
            exec_instances: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl Task for KrunTask {
    async fn create(
        &self,
        _ctx: &TtrpcContext,
        req: CreateTaskRequest,
    ) -> ttrpc::Result<CreateTaskResponse> {
        let id = req.id().to_string();
        let bundle = req.bundle().to_string();
        let stdin = req.stdin().to_string();
        let stdout = req.stdout().to_string();
        let stderr = req.stderr().to_string();
        let terminal = req.terminal();

        let builder = MicroVmBuilder::from_bundle(&bundle)
            .map_err(|e| {
                ttrpc::Error::RpcStatus(ttrpc::get_status(
                    ttrpc::Code::INVALID_ARGUMENT,
                    e.to_string(),
                ))
            })?
            .detach(true);

        let vm = builder.run().await.map_err(|e| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::INTERNAL, e.to_string()))
        })?;

        let pid = vm.pid().unwrap_or(0);
        let log_path = vm.console_log_path();

        let stream_handle = if !stdout.is_empty() || !stderr.is_empty() {
            let stdout_clone = stdout.clone();
            let stderr_clone = stderr.clone();
            Some(tokio::spawn(async move {
                stream_console_to_fifos(log_path, stdout_clone, stderr_clone, pid).await;
            }))
        } else {
            None
        };

        let vm_arc = Arc::new(Mutex::new(vm));
        let instance = TaskInstance {
            id: id.clone(),
            pid,
            bundle: PathBuf::from(bundle),
            stdin,
            stdout,
            stderr,
            terminal,
            status: Status::CREATED,
            exit_status: None,
            exited_at: None,
            vm: Some(vm_arc),
            stream_handle,
        };

        self.instances.lock().await.insert(id, instance);

        let mut resp = CreateTaskResponse::new();
        resp.set_pid(pid);
        Ok(resp)
    }

    async fn start(&self, _ctx: &TtrpcContext, req: StartRequest) -> ttrpc::Result<StartResponse> {
        let req_id = req.id();
        {
            let mut execs = self.exec_instances.lock().await;
            if let Some(exec_inst) = execs.get_mut(req_id) {
                let instances = self.instances.lock().await;
                let parent = instances.get(&exec_inst.task_id).ok_or_else(|| {
                    ttrpc::Error::RpcStatus(ttrpc::get_status(
                        ttrpc::Code::NOT_FOUND,
                        format!("parent task {} not found", exec_inst.task_id),
                    ))
                })?;

                let rootfs = parent.bundle.join("rootfs");
                let mut exec_req = microvm_core::ExecRequest::new(exec_inst.cmd.clone())
                    .with_env(exec_inst.env.clone())
                    .with_tty(exec_inst.terminal);
                if let Some(ref w) = exec_inst.workdir {
                    exec_req = exec_req.with_workdir(w.clone());
                }

                let stdout_fifo = exec_inst.stdout.clone();
                let stderr_fifo = exec_inst.stderr.clone();

                let exec_res = microvm_core::exec_in_guest_rootfs(&rootfs, &exec_req).await;
                let (code, out, err) = match exec_res {
                    Ok(resp) => (resp.exit_code as u32, resp.stdout, resp.stderr),
                    Err(e) => (126, String::new(), e.to_string()),
                };

                if !stdout_fifo.is_empty() && !out.is_empty() {
                    let _ = tokio::fs::write(&stdout_fifo, out.as_bytes()).await;
                }
                if !stderr_fifo.is_empty() && !err.is_empty() {
                    let _ = tokio::fs::write(&stderr_fifo, err.as_bytes()).await;
                }

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                let exited_at = Timestamp {
                    seconds: now.as_secs() as i64,
                    nanos: now.subsec_nanos() as i32,
                    ..Default::default()
                };

                exec_inst.status = Status::STOPPED;
                exec_inst.exit_status = Some(code);
                exec_inst.exited_at = Some(exited_at);

                let mut resp = StartResponse::new();
                resp.set_pid(std::process::id());
                return Ok(resp);
            }
        }

        let mut instances = self.instances.lock().await;
        let instance = instances.get_mut(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        instance.status = Status::RUNNING;
        let mut resp = StartResponse::new();
        resp.set_pid(instance.pid);
        Ok(resp)
    }

    async fn state(&self, _ctx: &TtrpcContext, req: StateRequest) -> ttrpc::Result<StateResponse> {
        let req_id = req.id();
        {
            let execs = self.exec_instances.lock().await;
            if let Some(exec_inst) = execs.get(req_id) {
                let mut resp = StateResponse::new();
                resp.set_id(exec_inst.exec_id.clone());
                resp.set_pid(std::process::id());
                resp.set_status(exec_inst.status);
                resp.set_stdin(exec_inst.stdin.clone());
                resp.set_stdout(exec_inst.stdout.clone());
                resp.set_stderr(exec_inst.stderr.clone());
                resp.set_terminal(exec_inst.terminal);
                if let Some(code) = exec_inst.exit_status {
                    resp.set_exit_status(code);
                }
                if let Some(ref ts) = exec_inst.exited_at {
                    resp.set_exited_at(ts.clone());
                }
                return Ok(resp);
            }
        }

        let instances = self.instances.lock().await;
        let instance = instances.get(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        let is_alive = signal::kill(Pid::from_raw(instance.pid as i32), None).is_ok();
        let status = if is_alive {
            instance.status
        } else {
            Status::STOPPED
        };

        let mut resp = StateResponse::new();
        resp.set_id(instance.id.clone());
        resp.set_bundle(instance.bundle.to_string_lossy().to_string());
        resp.set_pid(instance.pid);
        resp.set_status(status);
        resp.set_stdin(instance.stdin.clone());
        resp.set_stdout(instance.stdout.clone());
        resp.set_stderr(instance.stderr.clone());
        resp.set_terminal(instance.terminal);
        if let Some(code) = instance.exit_status {
            resp.set_exit_status(code);
        }
        if let Some(ref ts) = instance.exited_at {
            resp.set_exited_at(ts.clone());
        }
        Ok(resp)
    }

    async fn kill(&self, _ctx: &TtrpcContext, req: KillRequest) -> ttrpc::Result<Empty> {
        let instances = self.instances.lock().await;
        let instance = instances.get(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        let sig = match Signal::try_from(req.signal() as i32) {
            Ok(s) => s,
            Err(_) => Signal::SIGTERM,
        };

        let _ = signal::kill(Pid::from_raw(instance.pid as i32), sig);
        Ok(Empty::new())
    }

    async fn wait(&self, _ctx: &TtrpcContext, req: WaitRequest) -> ttrpc::Result<WaitResponse> {
        let req_id = req.id();
        {
            let execs = self.exec_instances.lock().await;
            if let Some(exec_inst) = execs.get(req_id) {
                let code = exec_inst.exit_status.unwrap_or(0);
                let exited_at = exec_inst.exited_at.clone().unwrap_or_default();
                let mut resp = WaitResponse::new();
                resp.set_exit_status(code);
                resp.set_exited_at(exited_at);
                return Ok(resp);
            }
        }

        let (vm_opt, pid) = {
            let instances = self.instances.lock().await;
            let inst = instances.get(req.id()).ok_or_else(|| {
                ttrpc::Error::RpcStatus(ttrpc::get_status(
                    ttrpc::Code::NOT_FOUND,
                    format!("task {} not found", req.id()),
                ))
            })?;
            (inst.vm.clone(), inst.pid)
        };

        let exit_code = if let Some(vm_arc) = vm_opt {
            let mut vm_guard = vm_arc.lock().await;
            match vm_guard.wait().await {
                Ok(status) => status.code().unwrap_or(0) as u32,
                Err(_) => 137,
            }
        } else {
            while signal::kill(Pid::from_raw(pid as i32), None).is_ok() {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
            0
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let exited_at = Timestamp {
            seconds: now.as_secs() as i64,
            nanos: now.subsec_nanos() as i32,
            ..Default::default()
        };

        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get_mut(req.id()) {
            instance.status = Status::STOPPED;
            instance.exit_status = Some(exit_code);
            instance.exited_at = Some(exited_at.clone());
        }

        let mut resp = WaitResponse::new();
        resp.set_exit_status(exit_code);
        resp.set_exited_at(exited_at);
        Ok(resp)
    }

    async fn delete(
        &self,
        _ctx: &TtrpcContext,
        req: DeleteRequest,
    ) -> ttrpc::Result<DeleteResponse> {
        let req_id = req.id();
        {
            let mut execs = self.exec_instances.lock().await;
            if let Some(exec_inst) = execs.remove(req_id) {
                let mut resp = DeleteResponse::new();
                resp.set_pid(std::process::id());
                resp.set_exit_status(exec_inst.exit_status.unwrap_or(0));
                return Ok(resp);
            }
        }

        let mut instances = self.instances.lock().await;
        let instance = instances.remove(req.id());

        let mut pid = 0;
        let mut exit_code = 0;

        if let Some(inst) = instance {
            if let Some(handle) = inst.stream_handle {
                handle.abort();
            }
            pid = inst.pid;
            exit_code = inst.exit_status.unwrap_or(0);
            if let Some(vm_arc) = inst.vm {
                let vm_guard = vm_arc.lock().await;
                vm_guard.purge();
            }
        }

        let mut resp = DeleteResponse::new();
        resp.set_pid(pid);
        resp.set_exit_status(exit_code);
        Ok(resp)
    }

    async fn pids(&self, _ctx: &TtrpcContext, req: PidsRequest) -> ttrpc::Result<PidsResponse> {
        let instances = self.instances.lock().await;
        let instance = instances.get(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        let mut resp = PidsResponse::new();
        let mut proc_info = ProcessInfo::new();
        proc_info.pid = instance.pid;
        resp.processes.push(proc_info);
        Ok(resp)
    }

    async fn stats(&self, _ctx: &TtrpcContext, req: StatsRequest) -> ttrpc::Result<StatsResponse> {
        let instances = self.instances.lock().await;
        let instance = instances.get(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        let pid = instance.pid;
        let stats = crate::metrics::collect_process_stats(pid).unwrap_or_default();
        let cgroups_metrics = crate::metrics::build_cgroups_metrics(&stats);
        let any = crate::metrics::encode_metrics_any(&cgroups_metrics).map_err(|e| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::INTERNAL,
                format!("failed to serialize metrics: {}", e),
            ))
        })?;

        let mut resp = StatsResponse::new();
        resp.stats = containerd_shim_protos::protobuf::MessageField::some(any);
        Ok(resp)
    }

    async fn pause(&self, _ctx: &TtrpcContext, req: PauseRequest) -> ttrpc::Result<Empty> {
        let mut instances = self.instances.lock().await;
        let instance = instances.get_mut(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        if instance.pid > 0 {
            let _ = signal::kill(Pid::from_raw(instance.pid as i32), Signal::SIGSTOP);
        }
        instance.status = Status::PAUSED;
        Ok(Empty::new())
    }

    async fn resume(&self, _ctx: &TtrpcContext, req: ResumeRequest) -> ttrpc::Result<Empty> {
        let mut instances = self.instances.lock().await;
        let instance = instances.get_mut(req.id()).ok_or_else(|| {
            ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            ))
        })?;

        if instance.pid > 0 {
            let _ = signal::kill(Pid::from_raw(instance.pid as i32), Signal::SIGCONT);
        }
        instance.status = Status::RUNNING;
        Ok(Empty::new())
    }

    async fn exec(&self, _ctx: &TtrpcContext, req: ExecProcessRequest) -> ttrpc::Result<Empty> {
        let instances = self.instances.lock().await;
        if !instances.contains_key(req.id()) {
            return Err(ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            )));
        }

        let (cmd, env, workdir) = parse_process_spec(&req.spec);
        let exec_inst = ExecInstance {
            task_id: req.id().to_string(),
            exec_id: req.exec_id().to_string(),
            stdin: req.stdin().to_string(),
            stdout: req.stdout().to_string(),
            stderr: req.stderr().to_string(),
            terminal: req.terminal(),
            cmd: if cmd.is_empty() {
                vec!["/bin/sh".to_string()]
            } else {
                cmd
            },
            env,
            workdir,
            status: Status::CREATED,
            exit_status: None,
            exited_at: None,
        };

        self.exec_instances
            .lock()
            .await
            .insert(req.exec_id().to_string(), exec_inst);
        Ok(Empty::new())
    }

    async fn update(&self, _ctx: &TtrpcContext, req: UpdateTaskRequest) -> ttrpc::Result<Empty> {
        let instances = self.instances.lock().await;
        if !instances.contains_key(req.id()) {
            return Err(ttrpc::Error::RpcStatus(ttrpc::get_status(
                ttrpc::Code::NOT_FOUND,
                format!("task {} not found", req.id()),
            )));
        }
        Ok(Empty::new())
    }

    async fn shutdown(&self, _ctx: &TtrpcContext, _req: ShutdownRequest) -> ttrpc::Result<Empty> {
        self.exit.signal();
        Ok(Empty::new())
    }
}

fn parse_process_spec(
    spec: &containerd_shim_protos::protobuf::MessageField<
        containerd_shim_protos::protobuf::well_known_types::any::Any,
    >,
) -> (Vec<String>, Vec<String>, Option<String>) {
    if let Some(a) = spec.as_ref() {
        if let Ok(proc) = serde_json::from_slice::<oci_spec::runtime::Process>(&a.value) {
            let cmd = proc.args().clone().unwrap_or_default();
            let env = proc.env().clone().unwrap_or_default();
            let cwd = proc.cwd().to_string_lossy().to_string();
            let workdir = if cwd.is_empty() { None } else { Some(cwd) };
            return (cmd, env, workdir);
        }
    }
    (Vec::new(), Vec::new(), None)
}

async fn stream_console_to_fifos(
    log_path: PathBuf,
    stdout_fifo: String,
    stderr_fifo: String,
    pid: u32,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    tracing::info!(
        "Starting FIFO streamer for PID {}: stdout={:?}, stderr={:?}",
        pid,
        stdout_fifo,
        stderr_fifo
    );

    let mut stdout_writer = if !stdout_fifo.is_empty() {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .open(&stdout_fifo)
            .await
        {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("Failed to open stdout FIFO {}: {}", stdout_fifo, e);
                None
            }
        }
    } else {
        None
    };

    let stderr_writer = if !stderr_fifo.is_empty() && stderr_fifo != stdout_fifo {
        match tokio::fs::OpenOptions::new()
            .write(true)
            .open(&stderr_fifo)
            .await
        {
            Ok(f) => Some(f),
            Err(e) => {
                tracing::warn!("Failed to open stderr FIFO {}: {}", stderr_fifo, e);
                None
            }
        }
    } else {
        None
    };

    let wait_start = tokio::time::Instant::now();
    let log_file = loop {
        match tokio::fs::File::open(&log_path).await {
            Ok(f) => break Some(f),
            Err(_) => {
                if wait_start.elapsed() > tokio::time::Duration::from_secs(5) {
                    tracing::warn!(
                        "Timed out waiting for console log file at {}",
                        log_path.display()
                    );
                    break None;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        }
    };

    let Some(mut log_file) = log_file else {
        return;
    };

    let mut buf = [0u8; 8192];
    loop {
        match log_file.read(&mut buf).await {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                if let Some(ref mut writer) = stdout_writer {
                    if let Err(e) = writer.write_all(data).await {
                        tracing::debug!("stdout FIFO write terminated: {}", e);
                        stdout_writer = None;
                    } else {
                        let _ = writer.flush().await;
                    }
                }
            }
            Ok(_) => {
                let is_alive = signal::kill(Pid::from_raw(pid as i32), None).is_ok();
                if !is_alive {
                    if let Ok(remaining) = log_file.read(&mut buf).await {
                        if remaining > 0 {
                            if let Some(ref mut writer) = stdout_writer {
                                let _ = writer.write_all(&buf[..remaining]).await;
                                let _ = writer.flush().await;
                            }
                        }
                    }
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            Err(e) => {
                tracing::debug!("Error reading console log: {}", e);
                break;
            }
        }

        if stdout_writer.is_none() && stderr_writer.is_none() {
            break;
        }
    }

    tracing::debug!("FIFO streamer finished for PID {}", pid);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_process_spec() {
        let proc = oci_spec::runtime::ProcessBuilder::default()
            .args(vec!["/bin/echo".to_string(), "hello".to_string()])
            .env(vec!["FOO=BAR".to_string()])
            .cwd(PathBuf::from("/tmp"))
            .build()
            .unwrap();

        let json = serde_json::to_vec(&proc).unwrap();
        let any = containerd_shim_protos::protobuf::well_known_types::any::Any {
            type_url: "types.containerd.io/opencontainers/runtime-spec/1/Process".to_string(),
            value: json,
            ..Default::default()
        };
        let field = containerd_shim_protos::protobuf::MessageField::some(any);
        let (cmd, env, workdir) = parse_process_spec(&field);
        assert_eq!(cmd, vec!["/bin/echo", "hello"]);
        assert_eq!(env, vec!["FOO=BAR"]);
        assert_eq!(workdir.as_deref(), Some("/tmp"));
    }

    #[tokio::test]
    async fn test_task_pause_resume_lifecycle() {
        let task = KrunTask::new(Arc::new(ExitSignal::default()));
        let dummy_inst = TaskInstance {
            id: "task-1".to_string(),
            pid: 0,
            bundle: PathBuf::from("/tmp/bundle"),
            stdin: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            terminal: false,
            status: Status::RUNNING,
            exit_status: None,
            exited_at: None,
            vm: None,
            stream_handle: None,
        };
        task.instances
            .lock()
            .await
            .insert("task-1".to_string(), dummy_inst);

        let ctx = TtrpcContext {
            timeout_nano: 0,
            mh: containerd_shim_protos::ttrpc::MessageHeader::default(),
            metadata: HashMap::new(),
        };

        // Pause
        let mut pause_req = PauseRequest::new();
        pause_req.set_id("task-1".to_string());
        task.pause(&ctx, pause_req).await.unwrap();

        {
            let instances = task.instances.lock().await;
            assert_eq!(instances.get("task-1").unwrap().status, Status::PAUSED);
        }

        // Resume
        let mut resume_req = ResumeRequest::new();
        resume_req.set_id("task-1".to_string());
        task.resume(&ctx, resume_req).await.unwrap();

        {
            let instances = task.instances.lock().await;
            assert_eq!(instances.get("task-1").unwrap().status, Status::RUNNING);
        }
    }

    #[tokio::test]
    async fn test_task_exec_registration_state_delete() {
        let task = KrunTask::new(Arc::new(ExitSignal::default()));
        let dummy_inst = TaskInstance {
            id: "task-1".to_string(),
            pid: 0,
            bundle: PathBuf::from("/tmp/bundle"),
            stdin: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            terminal: false,
            status: Status::RUNNING,
            exit_status: None,
            exited_at: None,
            vm: None,
            stream_handle: None,
        };
        task.instances
            .lock()
            .await
            .insert("task-1".to_string(), dummy_inst);

        let ctx = TtrpcContext {
            timeout_nano: 0,
            mh: containerd_shim_protos::ttrpc::MessageHeader::default(),
            metadata: HashMap::new(),
        };

        // Register exec
        let mut exec_req = ExecProcessRequest::new();
        exec_req.set_id("task-1".to_string());
        exec_req.set_exec_id("exec-100".to_string());
        task.exec(&ctx, exec_req).await.unwrap();

        // State query for exec process
        let mut state_req = StateRequest::new();
        state_req.set_id("exec-100".to_string());
        let state_resp = task.state(&ctx, state_req).await.unwrap();
        assert_eq!(state_resp.id(), "exec-100");
        assert_eq!(state_resp.status(), Status::CREATED);

        // Delete exec process
        let mut del_req = DeleteRequest::new();
        del_req.set_id("exec-100".to_string());
        let del_resp = task.delete(&ctx, del_req).await.unwrap();
        assert_eq!(del_resp.exit_status(), 0);
    }

    #[tokio::test]
    async fn test_task_update() {
        let task = KrunTask::new(Arc::new(ExitSignal::default()));
        let dummy_inst = TaskInstance {
            id: "task-1".to_string(),
            pid: 0,
            bundle: PathBuf::from("/tmp/bundle"),
            stdin: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            terminal: false,
            status: Status::RUNNING,
            exit_status: None,
            exited_at: None,
            vm: None,
            stream_handle: None,
        };
        task.instances
            .lock()
            .await
            .insert("task-1".to_string(), dummy_inst);

        let ctx = TtrpcContext {
            timeout_nano: 0,
            mh: containerd_shim_protos::ttrpc::MessageHeader::default(),
            metadata: HashMap::new(),
        };

        let mut upd_req = UpdateTaskRequest::new();
        upd_req.set_id("task-1".to_string());
        assert!(task.update(&ctx, upd_req).await.is_ok());

        let mut not_found_req = UpdateTaskRequest::new();
        not_found_req.set_id("non-existent".to_string());
        assert!(task.update(&ctx, not_found_req).await.is_err());
    }
}
