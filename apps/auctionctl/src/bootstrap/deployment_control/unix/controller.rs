//! Unix 部署控制器的锁、配置预检、readiness、回滚和 receipt 操作。

use std::{
    fs::{self, OpenOptions},
    net::TcpListener,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::Context;
use auction_config::{AppConfig, Role};

use super::super::ControllerOptions;
use super::super::probe::{ReadinessProbe, readiness_probe};
use super::super::receipt::{self, RoleReceipt, StartupReceipt};
use super::super::systemd::{ManagedRole, SystemdClient, UnitSnapshot};
use super::model::{Controller, DeploymentLock, PreparedDeployment, RolePlan, StartFailure};
use super::validation::{
    canonical_now, ensure_unit_config, validate_config_dir, validate_deployment_config_file,
    validate_private_state_dir,
};
use crate::bootstrap::{
    DeploymentListener, admin_deployment_listener, deployment_check, public_deployment_listener,
};

const LOCK_FILE: &str = "deployment-control.lock";
const PROBE_INTERVAL: Duration = Duration::from_millis(200);

impl Controller {
    pub(in crate::bootstrap::deployment_control::unix) fn new(
        options: ControllerOptions,
    ) -> anyhow::Result<Self> {
        let state_dir = validate_private_state_dir(&options.state_dir)?;
        let systemd = SystemdClient::new(
            options.systemctl,
            options.worker_unit,
            options.admin_unit,
            options.public_unit,
        )?;
        Ok(Self { state_dir, systemd })
    }

    pub(in crate::bootstrap::deployment_control::unix) fn unit(&self, role: ManagedRole) -> &str {
        self.systemd.unit(role)
    }

    pub(in crate::bootstrap::deployment_control::unix) fn acquire_lock(
        &self,
    ) -> anyhow::Result<DeploymentLock> {
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

    pub(in crate::bootstrap::deployment_control::unix) fn prepare(
        &self,
        config_dir: &Path,
    ) -> anyhow::Result<PreparedDeployment> {
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

    pub(in crate::bootstrap::deployment_control::unix) fn snapshots(
        &self,
    ) -> anyhow::Result<std::collections::BTreeMap<ManagedRole, UnitSnapshot>> {
        self.systemd.snapshots()
    }

    pub(in crate::bootstrap::deployment_control::unix) fn snapshot(
        &self,
        role: ManagedRole,
    ) -> anyhow::Result<UnitSnapshot> {
        self.systemd.snapshot(role)
    }

    pub(in crate::bootstrap::deployment_control::unix) fn start_unit(
        &self,
        role: ManagedRole,
    ) -> anyhow::Result<()> {
        self.systemd.start_unit(role)
    }

    pub(in crate::bootstrap::deployment_control::unix) fn stop_unit(
        &self,
        role: ManagedRole,
    ) -> anyhow::Result<()> {
        self.systemd.stop_unit(role)
    }

    pub(in crate::bootstrap::deployment_control::unix) fn stop_all(&self) -> anyhow::Result<()> {
        self.systemd.stop_all()
    }

    pub(in crate::bootstrap::deployment_control::unix) fn launch(
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

    pub(in crate::bootstrap::deployment_control::unix) fn observe_ready_roles(
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

    pub(in crate::bootstrap::deployment_control::unix) fn ensure_addresses_available(
        &self,
        prepared: &PreparedDeployment,
    ) -> anyhow::Result<()> {
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

    pub(in crate::bootstrap::deployment_control::unix) fn publish_receipt(
        &self,
        mode: &str,
        roles: Vec<RoleReceipt>,
    ) -> anyhow::Result<()> {
        receipt::publish(&self.state_dir, mode, roles, canonical_now())
    }

    pub(in crate::bootstrap::deployment_control::unix) fn read_receipt(
        &self,
    ) -> anyhow::Result<Option<StartupReceipt>> {
        receipt::read(&self.state_dir)
    }

    pub(in crate::bootstrap::deployment_control::unix) fn remove_receipt(
        &self,
    ) -> anyhow::Result<()> {
        receipt::remove(&self.state_dir)
    }
}
