//! Unix 部署控制器的锁、配置预检、readiness、回滚和 receipt 操作。

use std::{
    fs::{self, OpenOptions},
    net::TcpListener,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;

use super::super::ControllerOptions;
use super::super::probe::{ReadinessProbe, readiness_probe};
use super::super::receipt::{self, RoleReceipt, StartupReceipt};
use super::super::systemd::{ManagedRole, SystemdClient, UnitSnapshot};
use super::model::{Controller, DeploymentLock, PreparedDeployment, RolePlan, StartFailure};
use super::validation::{
    canonical_now, ensure_unit_config, validate_config_dir, validate_private_state_dir,
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
        // 非阻塞文件锁只协调 auctionctl 控制操作；systemd 仍拥有 unit 自身的事务锁。
        if let Err(error) = file.try_lock() {
            match error {
                std::fs::TryLockError::WouldBlock => {
                    anyhow::bail!("另一个 auctionctl deployment 操作正在运行")
                }
                std::fs::TryLockError::Error(error) => {
                    return Err(error).context("获取部署控制锁失败");
                }
            }
        }
        Ok(DeploymentLock { file })
    }

    pub(in crate::bootstrap::deployment_control::unix) fn prepare(
        &self,
        config_dir: &Path,
    ) -> anyhow::Result<PreparedDeployment> {
        let config_dir = validate_config_dir(config_dir)?;
        let [public, admin, worker] =
            deployment_check(&config_dir, true).context("部署配置严格预检失败")?;

        let public_path = config_dir.join("public.toml");
        let admin_path = config_dir.join("admin.toml");
        let worker_path = config_dir.join("worker.toml");

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
            ensure_unit_config(role, self.unit(role), path, &snapshot)?;
            if role == ManagedRole::Admin
                && let Some(runtime_path) = admin.runtime_config_path.as_deref()
            {
                validate_runtime_service_owner(runtime_path, &snapshot)?;
            }
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

    pub(in crate::bootstrap::deployment_control::unix) async fn launch(
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
            let snapshot = match self.wait_ready(plan, startup_timeout).await {
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
        if let Err(error) = self.verify_receipt_instances(prepared, &receipts) {
            return Err(
                self.rollback_failure(StartFailure::new("startup-receipt", error), &started)
            );
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

    pub(in crate::bootstrap::deployment_control::unix) async fn observe_ready_roles(
        &self,
        prepared: &PreparedDeployment,
        startup_timeout: Duration,
    ) -> anyhow::Result<Vec<RoleReceipt>> {
        let mut receipts = Vec::with_capacity(prepared.plans.len());
        let mut previous_started = 0;
        for plan in &prepared.plans {
            let snapshot = self.wait_ready(plan, startup_timeout).await?;
            if snapshot.started_monotonic_micros == 0
                || (previous_started > 0 && snapshot.started_monotonic_micros <= previous_started)
            {
                anyhow::bail!("现有 unit 的启动时间不符合 Worker → Admin → Public 顺序")
            }
            previous_started = snapshot.started_monotonic_micros;
            let observed = canonical_now();
            receipts.push(self.role_receipt(plan.role, snapshot, observed.clone(), observed));
        }
        self.verify_receipt_instances(prepared, &receipts)?;
        Ok(receipts)
    }

    async fn wait_ready(
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
            if snapshot.running() && readiness_probe(&plan.probe, deadline).await {
                // 探针前后的 systemd 进程身份必须一致；仅 PID 不能区分重启或复用。
                let current = self.snapshot(plan.role)?;
                if !current.running() || !snapshot.same_process(&current) {
                    anyhow::bail!("{} unit 在 readiness 探测期间退出或重启", plan.role.name());
                }
                if Instant::now() < deadline {
                    println!(
                        "role={} unit={} readiness=ready main_pid={}",
                        plan.role.name(),
                        self.unit(plan.role),
                        current.main_pid,
                    );
                    return Ok(current);
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "{} 未在 {} 秒内通过应用 readiness；请查看 `systemctl status {}` 与 journald",
                    plan.role.name(),
                    startup_timeout.as_secs(),
                    self.unit(plan.role),
                )
            }
            tokio::time::sleep(
                PROBE_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }

    /// 发布前复核各角色仍是已通过探测的代次；探测本身不证明 socket 的 PID 归属。
    fn verify_receipt_instances(
        &self,
        prepared: &PreparedDeployment,
        receipts: &[RoleReceipt],
    ) -> anyhow::Result<()> {
        if receipts.len() != prepared.plans.len() {
            anyhow::bail!("启动回执缺少角色记录");
        }
        for (plan, receipt) in prepared.plans.iter().zip(receipts) {
            let snapshot = self.snapshot(plan.role)?;
            if !snapshot.running() || !receipt.matches(plan.role, self.unit(plan.role), &snapshot) {
                anyhow::bail!("{} unit 在 readiness 后退出或重启", plan.role.name());
            }
        }
        Ok(())
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

/// 原子替换要求固定的 Admin UID 同时拥有文件和可写父目录，不能拿运维调用者 UID 代替。
fn validate_runtime_service_owner(path: &Path, snapshot: &UnitSnapshot) -> anyhow::Result<()> {
    if snapshot.dynamic_user {
        anyhow::bail!(
            "Admin runtime 需要固定 owner；请将 Admin unit 改为固定 User 并关闭 DynamicUser，再配置 runtime 文件及父目录属主"
        );
    }
    let uid = service_uid(&snapshot.user)?;
    let metadata = fs::symlink_metadata(path).context("读取 Admin runtime 属主失败")?;
    let parent = path.parent().context("Admin runtime 父目录缺失")?;
    let parent_metadata =
        fs::symlink_metadata(parent).context("读取 Admin runtime 父目录属主失败")?;
    if metadata.uid() != uid
        || parent_metadata.uid() != uid
        || parent_metadata.permissions().mode() & 0o300 != 0o300
    {
        anyhow::bail!(
            "Admin runtime 文件和父目录必须归 Admin unit UID {uid} 拥有，且父目录须有 owner 写入/遍历权限：{}",
            path.display()
        );
    }
    Ok(())
}

fn service_uid(user: &str) -> anyhow::Result<u32> {
    // systemd 的空 User 按 root 运行，不按 auctionctl 的有效 UID 推断。
    if user.is_empty() {
        return Ok(0);
    }
    if let Ok(uid) = user.parse::<u32>() {
        return Ok(uid);
    }
    let user = std::ffi::CString::new(user).context("Admin unit User 包含非法字符")?;
    let mut buffer = vec![0_u8; 1024];
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: 输入为 NUL 结尾；record、buffer 和 result 在调用期间均有效且互不重叠。
        let status = unsafe {
            libc::getpwnam_r(
                user.as_ptr(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if status == libc::ERANGE && buffer.len() < 1024 * 1024 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if status != 0 || result.is_null() {
            anyhow::bail!("无法解析 Admin unit User；请配置可解析的固定服务账号");
        }
        // SAFETY: getpwnam_r 成功并返回非空 result，已完整初始化 record。
        return Ok(unsafe { record.assume_init().pw_uid });
    }
}
