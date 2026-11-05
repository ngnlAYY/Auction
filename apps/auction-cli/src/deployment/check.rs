//! 三角色联合预检：同一数据库目标、公共/runtime 文件、密钥拓扑、监听冲突与文件权限。
//! 预检返回已校验的配置快照，供启停编排与迁移直接消费，不在其后重新加载另一组文件。

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::Context;
use auction_config::{AppConfig, Role};
use auction_storage::{DatabaseBackend, strict_deployment_database_identity};
use serde::Serialize;

use super::{ConfigDir, Endpoint};

/// 通过联合校验的三角色配置与端点。
pub(crate) struct Deployment {
    pub(crate) admin: AppConfig,
    pub(crate) worker: AppConfig,
    pub(crate) public_endpoint: Endpoint,
    pub(crate) admin_endpoint: Endpoint,
    pub(crate) worker_control: Option<Endpoint>,
    paths: [PathBuf; 3],
    strict: bool,
    warnings: Vec<String>,
    database_budget: super::budget::DatabaseBudgetReport,
}

#[derive(Serialize)]
pub(crate) struct DeploymentReport<'a> {
    passed: bool,
    strict: bool,
    database_backend: &'static str,
    database_budget: &'a super::budget::DatabaseBudgetReport,
    roles: Vec<RoleReport<'a>>,
    worker_control: Option<&'a Endpoint>,
    runtime_config_file: Option<String>,
    warnings: &'a [String],
    backup: serde_json::Value,
    backup_heartbeat_success_configured: bool,
    backup_heartbeat_failure_configured: bool,
}

#[derive(Serialize)]
struct RoleReport<'a> {
    role: &'static str,
    config_file: String,
    listener: Option<&'a Endpoint>,
}

impl Deployment {
    pub(crate) fn report(&self) -> DeploymentReport<'_> {
        let listeners = [Some(&self.public_endpoint), Some(&self.admin_endpoint), None];
        DeploymentReport {
            passed: true,
            strict: self.strict,
            database_backend: DatabaseBackend::Postgres.name(),
            database_budget: &self.database_budget,
            roles: [Role::Public, Role::Admin, Role::Worker]
                .into_iter()
                .zip(&self.paths)
                .zip(listeners)
                .map(|((role, path), listener)| RoleReport {
                    role: role.name(),
                    config_file: path.display().to_string(),
                    listener,
                })
                .collect(),
            worker_control: self.worker_control.as_ref(),
            runtime_config_file: self
                .worker
                .runtime_config_path
                .as_ref()
                .map(|path| path.display().to_string()),
            warnings: &self.warnings,
            backup: super::summary::backup_projection(&self.worker),
            backup_heartbeat_success_configured: self
                .worker
                .backup_heartbeat_success_url()
                .is_some(),
            backup_heartbeat_failure_configured: self
                .worker
                .backup_heartbeat_failure_url()
                .is_some(),
        }
    }
}

/// 严格模式下权限问题直接失败；非严格模式只收集警告，便于检查示例目录。
pub(crate) fn check(config_dir: &ConfigDir, strict: bool) -> anyhow::Result<Deployment> {
    let mut permissions = Permissions { strict, warnings: Vec::new() };
    let directory = config_dir.path();
    let metadata = fs::symlink_metadata(directory)
        .with_context(|| format!("读取配置目录失败：{}", directory.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!("配置目录必须是非符号链接目录：{}", directory.display());
    }
    permissions.not_writable_by_others(directory, "配置目录", &metadata)?;

    let mut configs = Vec::with_capacity(3);
    let mut paths = Vec::with_capacity(3);
    for role in [Role::Public, Role::Admin, Role::Worker] {
        let path = config_dir.role_path(role);
        let metadata = regular_file(&path, "角色配置")?;
        permissions.not_writable_by_others(&path, "角色配置", &metadata)?;
        let config = config_dir.load(role)?;
        let common = config.common_config_path.as_deref().context("角色配置缺少 common_file")?;
        let metadata = regular_file(common, "公共配置")?;
        permissions.not_writable_by_others(common, "公共配置", &metadata)?;
        let secrets = config.secrets_path.as_deref().context("角色配置缺少 secrets_file")?;
        let metadata = regular_file(secrets, "敏感配置")?;
        permissions.owner_only(secrets, &metadata)?;
        if let Some(runtime) = config.runtime_config_path.as_deref() {
            let metadata = regular_file(runtime, "runtime 配置")?;
            permissions.runtime(runtime, &metadata)?;
        }
        let url =
            config.database_url().with_context(|| format!("{} 配置缺少数据库连接", role.name()))?;
        if DatabaseBackend::from_url(url)? != DatabaseBackend::Postgres {
            anyhow::bail!("部署只支持 PostgreSQL");
        }
        configs.push(config);
        paths.push(path);
    }
    let [public, admin, worker]: [AppConfig; 3] =
        configs.try_into().map_err(|_| anyhow::anyhow!("部署必须包含三个角色"))?;

    permissions.warnings.extend(admin.admin_security_warnings().into_iter().map(str::to_owned));
    shared_files(&public, &admin, &worker)?;
    shared_database(&public, &admin, &worker)?;
    database_accounts(&public, &admin, &worker, &mut permissions)?;
    shared_runtime(&public, &admin, &worker)?;
    admin_worker(&admin, &worker)?;
    worker.validate_backup()?;
    let database_budget = super::budget::check(&public, &admin, &worker)?;

    let public_endpoint = Endpoint::public(&public)?;
    let admin_endpoint = Endpoint::admin(&admin)?;
    let worker_control = Endpoint::worker_control(&worker)?;
    for endpoint in [&public_endpoint, &admin_endpoint] {
        if let Endpoint::Tcp(address) = endpoint
            && address.port() == 0
        {
            anyhow::bail!("部署监听不能使用动态端口 0");
        }
        if worker_control.as_ref().is_some_and(|control| control.conflicts_with(endpoint)) {
            anyhow::bail!("HTTP Unix listener 不能复用 Worker control socket");
        }
    }
    if public_endpoint.conflicts_with(&admin_endpoint) {
        anyhow::bail!("Public 与 Admin 的监听端点不能重叠");
    }
    let paths: [PathBuf; 3] =
        paths.try_into().map_err(|_| anyhow::anyhow!("部署必须包含三个角色"))?;
    for warning in &permissions.warnings {
        crate::app::progress(format_args!("警告：{warning}"));
    }
    Ok(Deployment {
        admin,
        worker,
        public_endpoint,
        admin_endpoint,
        worker_control,
        paths,
        strict,
        warnings: permissions.warnings,
        database_budget,
    })
}

fn shared_files(public: &AppConfig, admin: &AppConfig, worker: &AppConfig) -> anyhow::Result<()> {
    let common = canonical(public.common_config_path.as_deref())?;
    let runtime = canonical(public.runtime_config_path.as_deref())?;
    for config in [admin, worker] {
        if canonical(config.common_config_path.as_deref())? != common {
            anyhow::bail!("Public、Admin 与 Worker 必须引用同一份 common_file");
        }
        if canonical(config.runtime_config_path.as_deref())? != runtime {
            anyhow::bail!("Public、Admin 与 Worker 必须引用同一份 runtime_config_file，或全部省略");
        }
    }
    Ok(())
}

/// 数据库账号分工：服务只能使用迁移 081 授权的运行账号，不得使用迁移 owner。
/// 严格模式缺 `[database_roles] owner` 声明即失败，因为离线无法证明两者分离。
fn database_accounts(
    public: &AppConfig,
    admin: &AppConfig,
    worker: &AppConfig,
    permissions: &mut Permissions,
) -> anyhow::Result<()> {
    if let Err(error) = public.database_roles.require_owner_declared() {
        permissions.report(error.to_string())?;
    }
    for config in [public, admin, worker] {
        let url = config
            .database_url()
            .with_context(|| format!("{} 配置缺少数据库连接", config.role.name()))?;
        let username = auction_config::database_url_username(url)
            .with_context(|| format!("{} 的数据库连接缺少显式账号名", config.role.name()))?;
        if let Some(problem) = config.database_roles.service_account_problem(&username) {
            permissions.report(format!("{}：{problem}", config.role.name()))?;
        }
    }
    Ok(())
}

fn shared_database(
    public: &AppConfig,
    admin: &AppConfig,
    worker: &AppConfig,
) -> anyhow::Result<()> {
    let identity = |config: &AppConfig| {
        strict_deployment_database_identity(config.database_url().unwrap_or_default())
            .context("确认部署数据库目标失败")
    };
    let expected = identity(public)?;
    if identity(admin)? != expected || identity(worker)? != expected {
        anyhow::bail!("Public、Admin 与 Worker 必须指向同一个数据库目标");
    }
    for config in [admin, worker] {
        if config.equipment_data_key_id() != public.equipment_data_key_id()
            || config.equipment_data_key() != public.equipment_data_key()
        {
            anyhow::bail!("Public、Admin 与 Worker 必须使用相同的 equipment_data_key 标识和材料");
        }
    }
    Ok(())
}

/// 同一路径可能在三次加载之间被 Admin 原子替换，规则快照也必须逐项一致。
fn shared_runtime(public: &AppConfig, admin: &AppConfig, worker: &AppConfig) -> anyhow::Result<()> {
    if admin.equipment_colors != public.equipment_colors
        || worker.equipment_colors != public.equipment_colors
    {
        anyhow::bail!("三角色加载的装备分组与论坛标色规则不一致，请在配置稳定后重试");
    }
    let admin_bot_players =
        admin.admin_runtime.as_ref().map(|runtime| runtime.bot_players.as_slice()).unwrap_or(&[]);
    if public.bot_players != worker.bot_players || worker.bot_players != admin_bot_players {
        anyhow::bail!("三角色加载的 bot_players 名单不一致，请在配置稳定后重试");
    }
    Ok(())
}

fn admin_worker(admin: &AppConfig, worker: &AppConfig) -> anyhow::Result<()> {
    if admin.auction != worker.auction {
        anyhow::bail!("Admin 与 Worker 的有效拍卖模板或排期不一致");
    }
    if admin.worker_control_socket_path() != worker.worker.control.socket_path {
        anyhow::bail!("Admin 与 Worker 必须配置相同的 Worker control socket_path");
    }
    if let Some(path) = &admin.worker_config_path {
        let worker_path = worker.config_path.as_deref().context("Worker 主配置路径缺失")?;
        if fs::canonicalize(path).context("读取 Admin worker_config_file 失败")?
            != fs::canonicalize(worker_path).context("读取 Worker 主配置失败")?
        {
            anyhow::bail!("Admin worker_config_file 必须指向当前部署的 Worker 主配置");
        }
    }
    if admin.user_login_data_key_id() != worker.user_login_data_key_id()
        || admin.user_login_data_key() != worker.user_login_data_key()
    {
        anyhow::bail!("Admin 与 Worker 必须使用相同的 user_login_data_key 标识和材料");
    }
    if admin.credential_data_key_id() != worker.credential_data_key_id()
        || admin.credential_data_key() != worker.credential_data_key()
    {
        anyhow::bail!("Admin 与 Worker 必须使用相同的 credential_data_key 标识和材料");
    }
    Ok(())
}

fn canonical(path: Option<&Path>) -> anyhow::Result<Option<PathBuf>> {
    path.map(|path| {
        fs::canonicalize(path).with_context(|| format!("解析配置文件失败：{}", path.display()))
    })
    .transpose()
}

fn regular_file(path: &Path, kind: &str) -> anyhow::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取{kind}失败：{}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        anyhow::bail!("{kind}必须是非符号链接的普通文件：{}", path.display());
    }
    Ok(metadata)
}

struct Permissions {
    strict: bool,
    warnings: Vec<String>,
}

impl Permissions {
    fn report(&mut self, message: String) -> anyhow::Result<()> {
        if self.strict {
            anyhow::bail!(message);
        }
        self.warnings.push(message);
        Ok(())
    }

    fn not_writable_by_others(
        &mut self,
        path: &Path,
        kind: &str,
        metadata: &fs::Metadata,
    ) -> anyhow::Result<()> {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o022 != 0 {
            self.report(format!(
                "{kind}不得允许 group/other 写入（{mode:o}）：{}",
                path.display()
            ))?;
        }
        Ok(())
    }

    fn owner_only(&mut self, path: &Path, metadata: &fs::Metadata) -> anyhow::Result<()> {
        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            self.report(format!("敏感配置必须仅 owner 可访问（{mode:o}）：{}", path.display()))?;
        }
        Ok(())
    }

    /// Admin 以原子替换保存 runtime，文件须为 0640 且父目录不得允许他人写入。
    fn runtime(&mut self, path: &Path, file_metadata: &fs::Metadata) -> anyhow::Result<()> {
        let parent = path.parent().context("runtime 配置缺少父目录")?;
        let metadata = fs::symlink_metadata(parent)
            .with_context(|| format!("读取 runtime 父目录失败：{}", parent.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            anyhow::bail!("runtime 配置父目录必须是非符号链接目录：{}", parent.display());
        }
        self.not_writable_by_others(parent, "runtime 配置父目录", &metadata)?;
        let mode = file_metadata.permissions().mode() & 0o777;
        if mode != 0o640 {
            self.report(format!("runtime 配置必须使用 0640 权限（{mode:o}）：{}", path.display()))?;
        }
        Ok(())
    }
}
