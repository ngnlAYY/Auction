//! Unix/systemd 部署控制器的生命周期实现。

use std::{
    fs::{self, File, OpenOptions},
    net::TcpListener,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use super::ControllerOptions;
use super::probe::{ReadinessProbe, readiness_probe};
use super::receipt::{self, RoleReceipt, StartupReceipt, receipt_match};
use super::systemd::{ManagedRole, START_ORDER, SystemdClient, UnitSnapshot, single_line};
use crate::bootstrap::{
    DeploymentListener, admin_deployment_listener, deployment_check, migrate_config,
    public_deployment_listener,
};
use anyhow::Context;
use auction_config::{AppConfig, Role};
use chrono::{SecondsFormat, Utc};

const LOCK_FILE: &str = "deployment-control.lock";
const PROBE_INTERVAL: Duration = Duration::from_millis(200);
struct Controller {
    state_dir: PathBuf,
    systemd: SystemdClient,
}

struct DeploymentLock {
    file: File,
}

impl Drop for DeploymentLock {
    fn drop(&mut self) {
        // 文件描述符关闭也会释放锁；显式 unlock 让失败路径的所有权更容易审计。
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

struct PreparedDeployment {
    migration_config: AppConfig,
    plans: Vec<RolePlan>,
}

struct RolePlan {
    role: ManagedRole,
    probe: ReadinessProbe,
}

struct StartFailure {
    code: &'static str,
    error: anyhow::Error,
}

impl StartFailure {
    fn new(code: &'static str, error: impl Into<anyhow::Error>) -> Self {
        Self { code, error: error.into() }
    }
}

pub(super) async fn start(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    let result = start_inner(options, config_dir, startup_timeout).await;
    finish_start_result(result)
}

async fn start_inner(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> Result<(), StartFailure> {
    let controller =
        Controller::new(options).map_err(|error| StartFailure::new("start-prepare", error))?;
    let _lock =
        controller.acquire_lock().map_err(|error| StartFailure::new("start-lock", error))?;
    let prepared = controller
        .prepare(&config_dir)
        .map_err(|error| StartFailure::new("start-prepare", error))?;
    let snapshots =
        controller.snapshots().map_err(|error| StartFailure::new("start-state", error))?;
    let running = snapshots.values().filter(|snapshot| snapshot.running()).count();
    if running == START_ORDER.len() {
        let records = controller
            .observe_ready_roles(&prepared, startup_timeout)
            .map_err(|error| StartFailure::new("start-state", error))?;
        controller
            .publish_receipt("adopted", records)
            .map_err(|error| StartFailure::new("startup-receipt", error))?;
        println!("deployment already running; readiness and startup receipt refreshed");
        return Ok(());
    }
    if snapshots.values().any(|snapshot| !snapshot.stopped()) {
        return Err(StartFailure::new(
            "start-state",
            anyhow::anyhow!(
                "检测到部分 unit 正在运行或转换状态；请先执行 `auctionctl deployment stop`"
            ),
        ));
    }
    controller
        .ensure_addresses_available(&prepared)
        .map_err(|error| StartFailure::new("address-check", error))?;
    migrate_config(&prepared.migration_config)
        .await
        .map_err(|error| StartFailure::new("migration", error))?;
    controller.launch(&prepared, startup_timeout)
}

pub(super) fn stop(options: ControllerOptions) -> anyhow::Result<()> {
    let controller = Controller::new(options)?;
    let _lock = controller.acquire_lock()?;
    controller.stop_all()?;
    controller.remove_receipt()?;
    println!("deployment stopped");
    Ok(())
}

pub(super) async fn restart(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> anyhow::Result<()> {
    let result = restart_inner(options, config_dir, startup_timeout).await;
    finish_start_result(result)
}

async fn restart_inner(
    options: ControllerOptions,
    config_dir: PathBuf,
    startup_timeout: Duration,
) -> Result<(), StartFailure> {
    let controller =
        Controller::new(options).map_err(|error| StartFailure::new("start-prepare", error))?;
    let _lock =
        controller.acquire_lock().map_err(|error| StartFailure::new("start-lock", error))?;
    // 与旧版一致，先证明新配置和 unit 契约可用，再停止当前服务。
    let prepared = controller
        .prepare(&config_dir)
        .map_err(|error| StartFailure::new("start-prepare", error))?;
    controller.stop_all().map_err(|error| StartFailure::new("restart-stop", error))?;
    controller.remove_receipt().map_err(|error| StartFailure::new("restart-stop", error))?;
    controller
        .ensure_addresses_available(&prepared)
        .map_err(|error| StartFailure::new("address-check", error))?;
    migrate_config(&prepared.migration_config)
        .await
        .map_err(|error| StartFailure::new("migration", error))?;
    controller.launch(&prepared, startup_timeout)
}

pub(super) fn status(options: ControllerOptions) -> anyhow::Result<()> {
    let controller = Controller::new(options)?;
    let receipt = controller.read_receipt()?;
    let mut healthy = true;
    for role in START_ORDER {
        match controller.snapshot(role) {
            Ok(snapshot) => {
                let receipt_state = receipt
                    .as_ref()
                    .map(|receipt| receipt_match(receipt, role, controller.unit(role), &snapshot))
                    .unwrap_or("missing");
                println!(
                    "role={} unit={} load_state={} active_state={} sub_state={} main_pid={} receipt={}",
                    role.name(),
                    controller.unit(role),
                    snapshot.load_state,
                    snapshot.active_state,
                    snapshot.sub_state,
                    snapshot.main_pid,
                    receipt_state,
                );
                if !snapshot.running() || receipt_state == "stale" {
                    healthy = false;
                }
            }
            Err(error) => {
                healthy = false;
                println!(
                    "role={} unit={} load_state=unknown active_state=unknown error={}",
                    role.name(),
                    controller.unit(role),
                    single_line(&error.to_string(), 512),
                );
            }
        }
    }
    if healthy { Ok(()) } else { anyhow::bail!("deployment status is not fully healthy") }
}

fn finish_start_result(result: Result<(), StartFailure>) -> anyhow::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(failure) => {
            eprintln!("[auction] start-failure-v1={}", failure.code);
            Err(failure.error)
        }
    }
}

impl Controller {
    fn new(options: ControllerOptions) -> anyhow::Result<Self> {
        let state_dir = validate_private_state_dir(&options.state_dir)?;
        let systemd = SystemdClient::new(
            options.systemctl,
            options.worker_unit,
            options.admin_unit,
            options.public_unit,
        )?;
        Ok(Self { state_dir, systemd })
    }

    fn unit(&self, role: ManagedRole) -> &str {
        self.systemd.unit(role)
    }

    fn acquire_lock(&self) -> anyhow::Result<DeploymentLock> {
        let path = self.state_dir.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let file = options
            .open(&path)
            .with_context(|| format!("打开部署控制锁失败：{}", path.display()))?;
        let metadata = file.metadata().context("读取部署控制锁元数据失败")?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            anyhow::bail!("部署控制锁必须是单链接普通文件")
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .context("收紧部署控制锁权限失败")?;
        }
        // 非阻塞 flock 只协调 auctionctl 控制操作；systemd 仍拥有 unit 自身的事务锁。
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                anyhow::bail!("另一个 auctionctl deployment 操作正在运行")
            }
            return Err(error).context("获取部署控制锁失败");
        }
        Ok(DeploymentLock { file })
    }

    fn prepare(&self, config_dir: &Path) -> anyhow::Result<PreparedDeployment> {
        let config_dir = validate_config_dir(config_dir)?;
        deployment_check(&config_dir, true).context("部署配置严格预检失败")?;

        let public_path = config_dir.join("public.toml");
        let admin_path = config_dir.join("admin.toml");
        let worker_path = config_dir.join("worker.toml");
        for path in [&public_path, &admin_path, &worker_path] {
            validate_deployment_config_file(path)?;
        }
        let public = AppConfig::load_from_path(Role::Public, &public_path)
            .context("加载 Public 部署配置失败")?;
        let admin = AppConfig::load_from_path(Role::Admin, &admin_path)
            .context("加载 Admin 部署配置失败")?;
        let worker = AppConfig::load_from_path(Role::Worker, &worker_path)
            .context("加载 Worker 部署配置失败")?;

        let public_listener = public_deployment_listener(&public)?;
        let admin_probe = match admin_deployment_listener(&admin)? {
            DeploymentListener::Tcp(address) => ReadinessProbe::Tcp(address),
            DeploymentListener::Unix(path) => ReadinessProbe::UnixHttp(path),
            DeploymentListener::None => anyhow::bail!("Admin 部署配置缺少监听器"),
        };
        let public_probe = match public_listener {
            DeploymentListener::Tcp(address) => {
                if address.port() == 0 {
                    anyhow::bail!("部署控制不接受动态端口 0")
                }
                ReadinessProbe::Tcp(address)
            }
            DeploymentListener::Unix(path) => ReadinessProbe::UnixHttp(path),
            DeploymentListener::None => anyhow::bail!("Public 部署配置缺少监听器"),
        };
        if !worker.worker.control.enabled {
            anyhow::bail!("deployment start 要求启用 worker.control，以验证 Worker readiness")
        }
        let worker_socket = PathBuf::from(&worker.worker.control.socket_path);
        if !worker_socket.is_absolute() {
            anyhow::bail!("Worker control socket 必须是绝对路径")
        }

        for (role, path) in [
            (ManagedRole::Worker, &worker_path),
            (ManagedRole::Admin, &admin_path),
            (ManagedRole::Public, &public_path),
        ] {
            let snapshot = self.snapshot(role)?;
            if snapshot.load_state != "loaded" {
                anyhow::bail!("{} unit 未装载：{}", role.name(), self.unit(role))
            }
            ensure_unit_config(role, self.unit(role), path, &snapshot.environment)?;
        }

        Ok(PreparedDeployment {
            migration_config: public,
            plans: vec![
                RolePlan {
                    role: ManagedRole::Worker,
                    probe: ReadinessProbe::UnixControl(worker_socket),
                },
                RolePlan { role: ManagedRole::Admin, probe: admin_probe },
                RolePlan { role: ManagedRole::Public, probe: public_probe },
            ],
        })
    }

    fn snapshots(&self) -> anyhow::Result<std::collections::BTreeMap<ManagedRole, UnitSnapshot>> {
        self.systemd.snapshots()
    }

    fn snapshot(&self, role: ManagedRole) -> anyhow::Result<UnitSnapshot> {
        self.systemd.snapshot(role)
    }

    fn start_unit(&self, role: ManagedRole) -> anyhow::Result<()> {
        self.systemd.start_unit(role)
    }

    fn stop_unit(&self, role: ManagedRole) -> anyhow::Result<()> {
        self.systemd.stop_unit(role)
    }

    fn stop_all(&self) -> anyhow::Result<()> {
        self.systemd.stop_all()
    }

    fn launch(
        &self,
        prepared: &PreparedDeployment,
        startup_timeout: Duration,
    ) -> Result<(), StartFailure> {
        let mut started = Vec::new();
        let mut receipts = Vec::new();
        for plan in &prepared.plans {
            let observed_started_at = canonical_now();
            // 即使 systemctl 返回错误，unit job 也可能已经进入队列，因此先纳入回滚集合。
            started.push(plan.role);
            if let Err(error) = self.start_unit(plan.role) {
                return Err(self
                    .rollback_failure(StartFailure::new(plan.role.start_code(), error), &started));
            }
            let snapshot = match self.wait_ready(plan, startup_timeout) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return Err(self.rollback_failure(
                        StartFailure::new(plan.role.readiness_code(), error),
                        &started,
                    ));
                }
            };
            receipts.push(self.role_receipt(
                plan.role,
                snapshot,
                observed_started_at,
                canonical_now(),
            ));
        }
        if let Err(error) = self.publish_receipt("started", receipts) {
            return Err(
                self.rollback_failure(StartFailure::new("startup-receipt", error), &started)
            );
        }
        println!("deployment started order=worker,admin,public");
        Ok(())
    }

    fn rollback_failure(&self, mut failure: StartFailure, started: &[ManagedRole]) -> StartFailure {
        let mut rollback_failures = Vec::new();
        for role in started.iter().rev().copied() {
            if let Err(error) = self.stop_unit(role) {
                rollback_failures.push(format!("{}: {error:#}", role.name()));
            }
        }
        if let Err(error) = self.remove_receipt() {
            rollback_failures.push(format!("receipt: {error:#}"));
        }
        if !rollback_failures.is_empty() {
            failure.error = anyhow::anyhow!(
                "{:#}; 本次启动回滚不完整：{}",
                failure.error,
                rollback_failures.join(" | "),
            );
        }
        failure
    }

    fn observe_ready_roles(
        &self,
        prepared: &PreparedDeployment,
        startup_timeout: Duration,
    ) -> anyhow::Result<Vec<RoleReceipt>> {
        let mut receipts = Vec::with_capacity(prepared.plans.len());
        let mut previous_started = 0;
        for plan in &prepared.plans {
            let snapshot = self.wait_ready(plan, startup_timeout)?;
            if snapshot.started_monotonic_micros == 0
                || (previous_started > 0 && snapshot.started_monotonic_micros <= previous_started)
            {
                anyhow::bail!("现有 unit 的启动时间不符合 Worker → Admin → Public 顺序")
            }
            previous_started = snapshot.started_monotonic_micros;
            let observed = canonical_now();
            receipts.push(self.role_receipt(plan.role, snapshot, observed.clone(), observed));
        }
        Ok(receipts)
    }

    fn wait_ready(
        &self,
        plan: &RolePlan,
        startup_timeout: Duration,
    ) -> anyhow::Result<UnitSnapshot> {
        let deadline = Instant::now()
            .checked_add(startup_timeout)
            .ok_or_else(|| anyhow::anyhow!("startup timeout 溢出"))?;
        loop {
            let snapshot = self.snapshot(plan.role)?;
            if snapshot.load_state != "loaded" {
                anyhow::bail!("{} unit 不再处于 loaded 状态", plan.role.name())
            }
            if snapshot.active_state == "failed" {
                anyhow::bail!(
                    "{} unit 启动失败；请查看 `systemctl status {}` 与 journald",
                    plan.role.name(),
                    self.unit(plan.role),
                )
            }
            if snapshot.running() && readiness_probe(&plan.probe) {
                println!(
                    "role={} unit={} readiness=ready main_pid={}",
                    plan.role.name(),
                    self.unit(plan.role),
                    snapshot.main_pid,
                );
                return Ok(snapshot);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "{} 未在 {} 秒内通过应用 readiness；请查看 `systemctl status {}` 与 journald",
                    plan.role.name(),
                    startup_timeout.as_secs(),
                    self.unit(plan.role),
                )
            }
            thread::sleep(PROBE_INTERVAL);
        }
    }

    fn ensure_addresses_available(&self, prepared: &PreparedDeployment) -> anyhow::Result<()> {
        for plan in &prepared.plans {
            let ReadinessProbe::Tcp(address) = &plan.probe else {
                continue;
            };
            let listener = TcpListener::bind(address)
                .with_context(|| format!("{} 监听地址已被占用：{address}", plan.role.name()))?;
            drop(listener);
        }
        Ok(())
    }

    fn role_receipt(
        &self,
        role: ManagedRole,
        snapshot: UnitSnapshot,
        observed_started_at: String,
        observed_ready_at: String,
    ) -> RoleReceipt {
        RoleReceipt::new(role, self.unit(role), snapshot, observed_started_at, observed_ready_at)
    }

    fn publish_receipt(&self, mode: &str, roles: Vec<RoleReceipt>) -> anyhow::Result<()> {
        receipt::publish(&self.state_dir, mode, roles, canonical_now())
    }

    fn read_receipt(&self) -> anyhow::Result<Option<StartupReceipt>> {
        receipt::read(&self.state_dir)
    }

    fn remove_receipt(&self) -> anyhow::Result<()> {
        receipt::remove(&self.state_dir)
    }
}

fn validate_private_state_dir(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("部署控制状态目录必须是非根绝对路径")
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("规范化部署控制状态目录失败：{}", path.display()))?;
    if canonical != path {
        anyhow::bail!("部署控制状态目录必须是规范化、无符号链接的绝对路径")
    }
    let metadata = fs::symlink_metadata(&canonical).context("读取部署控制状态目录失败")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("部署控制状态目录必须是无符号链接的目录")
    }
    // systemd StateDirectory 与手工控制都要求操作者拥有目录，避免特权进程写入低权限用户可替换的路径。
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        anyhow::bail!("部署控制状态目录必须归当前有效用户所有")
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        anyhow::bail!("部署控制状态目录不得允许 group/other 访问；要求权限至多 0700")
    }
    Ok(canonical)
}

fn validate_config_dir(path: &Path) -> anyhow::Result<PathBuf> {
    if !path.is_absolute() || path == Path::new("/") {
        anyhow::bail!("deployment start 的 config-dir 必须是非根绝对路径")
    }
    let display = path.to_str().ok_or_else(|| anyhow::anyhow!("config-dir 必须是 UTF-8 路径"))?;
    if display.chars().any(|character| character.is_whitespace() || character.is_control()) {
        anyhow::bail!("config-dir 不能包含空白或控制字符")
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("规范化 config-dir 失败：{}", path.display()))?;
    let metadata = fs::symlink_metadata(&canonical).context("读取 config-dir 元数据失败")?;
    if canonical != path || !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("config-dir 必须是规范化、无符号链接的现有目录")
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!("config-dir 不得允许 group/other 写入")
    }
    Ok(canonical)
}

fn validate_deployment_config_file(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取部署主配置元数据失败：{}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("部署主配置必须是无符号链接的普通文件：{}", path.display())
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        anyhow::bail!("部署主配置不得允许 group/other 写入：{}", path.display())
    }
    Ok(())
}

fn ensure_unit_config(
    role: ManagedRole,
    unit: &str,
    config_path: &Path,
    environment: &str,
) -> anyhow::Result<()> {
    let expected = format!("AUCTION_CONFIG_FILE={}", config_path.display());
    let matches = environment
        .split_ascii_whitespace()
        .map(|entry| entry.trim_matches('"'))
        .any(|entry| entry == expected);
    if !matches {
        anyhow::bail!(
            "{} unit {} 未声明预期的 AUCTION_CONFIG_FILE={}；请先 daemon-reload",
            role.name(),
            unit,
            config_path.display(),
        )
    }
    Ok(())
}

fn canonical_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
