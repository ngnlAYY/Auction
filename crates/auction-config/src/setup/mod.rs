//! 首次部署的 PostgreSQL 配置物化边界。
//!
//! 输入基于通用配置模板生成 PostgreSQL 单机部署配置；敏感值不进入日志、错误正文或非敏感 TOML。运行进程仍由
//! `AppConfig` 加载这些文件，并通过相同的校验器阻止无效部署。

use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use toml::Value;
use tracing_subscriber::EnvFilter;
use url::Url;
use uuid::Uuid;

use auction_domain::{CREDIT_GRID, MAX_BID_AMOUNT};

use crate::{AppConfig, AuctionScheduleConfig, PostgresConnectionConfig, Role};

mod managed;
pub use managed::managed_runtime_directory;

/// 管理页提交、但不依赖浏览器 DTO 的一次性部署选项。
pub struct FirstRunConfig {
    pub database: PostgresConnectionConfig,
    pub admin_actor: String,
    pub admin_secret: String,
    pub public_origin: String,
    pub admin_origin: String,
    /// 正式 Public 回环 TCP 端口；必须非零且与 Admin 不同。
    pub public_listener_port: u16,
    /// 正式 Admin 回环 TCP 端口；同时用于角色 fallback 与 runtime。
    pub admin_listener_port: u16,
    pub metrics_enabled: bool,
    /// 新拍卖使用的周计划；首次设置即使关闭排期也必须保存合法候选。
    pub schedule: AuctionScheduleConfig,
    pub start_price: i64,
    pub log_filter: String,
    pub worker_concurrency: u32,
    pub worker_poll_interval_seconds: u64,
    pub realtime_poll_interval_millis: u64,
    pub control_group: String,
    pub control_uid: u32,
}

/// 首次部署的稳定失败分类；不包含输入内容或底层错误链。
#[derive(Debug, thiserror::Error)]
pub enum FirstRunError {
    #[error("初始化字段无效")]
    InvalidInput,
    #[error("初始化配置未通过运行时校验")]
    InvalidConfig,
    #[error("配置目标已存在")]
    AlreadyExists,
    #[error("配置文件写入失败")]
    Io,
    #[error("配置目录发布结果需要人工确认")]
    OutcomeUnknown,
}

impl FirstRunConfig {
    /// 在触碰磁盘或数据库前校验表单闭集和部署边界。
    pub fn validate(&self) -> Result<(), FirstRunError> {
        self.database.connection_url().map_err(|_| FirstRunError::InvalidInput)?;
        self.schedule.validate().map_err(|_| FirstRunError::InvalidInput)?;
        // 首次表单只接受规范的分钟精度时间；不改写正式 loader 既有的文本兼容范围。
        if self.schedule.timezone.len() > 128
            || [
                &self.schedule.preview_time,
                &self.schedule.open_time,
                &self.schedule.nominal_end_time,
            ]
            .iter()
            .any(|value| {
                !chrono::NaiveTime::parse_from_str(value, "%H:%M")
                    .is_ok_and(|time| time.format("%H:%M").to_string() == value.as_str())
            })
        {
            return Err(FirstRunError::InvalidInput);
        }
        if self.admin_actor.is_empty()
            || self.admin_actor.len() > 128
            || self.admin_actor.contains([':', '\r', '\n', '\0'])
            || !(16..=512).contains(&self.admin_secret.len())
            || self.admin_secret.contains(['\r', '\n', '\0'])
            || self.public_origin == self.admin_origin
            || !valid_origin(&self.public_origin)
            || !valid_origin(&self.admin_origin)
            || self.public_listener_port == 0
            || self.admin_listener_port == 0
            || self.public_listener_port == self.admin_listener_port
            || !(CREDIT_GRID..=MAX_BID_AMOUNT).contains(&self.start_price)
            || self.start_price % CREDIT_GRID != 0
            || self.log_filter.is_empty()
            || self.log_filter.len() > 256
            || self.log_filter.contains(['\r', '\n', '\0'])
            || EnvFilter::try_new(&self.log_filter).is_err()
            || !(1..=8).contains(&self.worker_concurrency)
            || !(11..=300).contains(&self.worker_poll_interval_seconds)
            || !(100..=5_000).contains(&self.realtime_poll_interval_millis)
            || self.control_group.is_empty()
            || self.control_group.len() > 64
            || !self
                .control_group
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            || !self
                .control_group
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(FirstRunError::InvalidInput);
        }
        Ok(())
    }

    /// 原子发布完整配置目录；已存在目录或文件绝不被首次设置覆盖。
    pub fn write_to(&self, destination: &Path) -> Result<(), FirstRunError> {
        self.write_directory(destination, None)
    }

    /// 合并部署仍原子生成三角色配置，仅将 Worker control 指向同级私有运行目录。
    pub fn write_managed_to(&self, destination: &Path) -> Result<(), FirstRunError> {
        let runtime = managed_runtime_directory(destination)?;
        self.write_directory(destination, Some(&runtime))
    }

    fn write_directory(
        &self,
        destination: &Path,
        runtime: Option<&Path>,
    ) -> Result<(), FirstRunError> {
        self.validate()?;
        ensure_destination_absent(destination)?;
        let parent = destination.parent().ok_or(FirstRunError::InvalidInput)?;
        if !parent.is_dir() || parent.is_symlink() {
            return Err(FirstRunError::InvalidInput);
        }
        let temporary = parent.join(format!(".auction-first-run-{}", Uuid::new_v4().simple()));
        // 创建瞬间就限制访问，不依赖后续 chmod 缩小目录权限。
        fs::DirBuilder::new().mode(0o700).create(&temporary).map_err(|_| FirstRunError::Io)?;
        let result = (|| {
            self.write_files(&temporary, runtime)?;
            for role in [Role::Public, Role::Admin, Role::Worker] {
                AppConfig::load_from_path(role, temporary.join(format!("{}.toml", role.name())))
                    .map_err(|_| FirstRunError::InvalidConfig)?;
            }
            ensure_destination_absent(destination)?;
            // 文件 fsync 不持久化目录项；先同步 runtime 子目录，再同步其父目录。
            for directory in [temporary.join("runtime"), temporary.clone()] {
                fs::File::open(directory)
                    .and_then(|directory| directory.sync_all())
                    .map_err(|_| FirstRunError::Io)?;
            }
            publish_directory(&temporary, destination)?;
            fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| FirstRunError::OutcomeUnknown)
        })();
        if result.is_err() {
            // 仅清理本次临时名称；发布后父目录同步失败也不能删除已出现的目标目录。
            let _ = fs::remove_dir_all(&temporary);
        }
        result
    }

    fn write_files(
        &self,
        directory: &Path,
        managed_runtime: Option<&Path>,
    ) -> Result<(), FirstRunError> {
        let mut common = template(include_str!("../../../../ops/config/examples/common.toml"))?;
        common["log_filter"] = Value::String(self.log_filter.clone());
        common["metrics"]["enabled"] = Value::Boolean(self.metrics_enabled);
        common["auction"]["schedule"]["enabled"] = Value::Boolean(self.schedule.enabled);
        for (key, value) in [
            ("timezone", &self.schedule.timezone),
            ("preview_day", &self.schedule.preview_day),
            ("preview_time", &self.schedule.preview_time),
            ("open_day", &self.schedule.open_day),
            ("open_time", &self.schedule.open_time),
            ("nominal_end_day", &self.schedule.nominal_end_day),
            ("nominal_end_time", &self.schedule.nominal_end_time),
        ] {
            common["auction"]["schedule"][key] = Value::String(value.clone());
        }
        common["auction"]["schedule"]["worlds"] =
            Value::Array(self.schedule.worlds.iter().cloned().map(Value::String).collect());
        common["auction"]["economics"]["start_price"] = Value::Integer(self.start_price);
        write_toml(directory.join("common.toml"), &common, 0o640)?;

        let mut public = template(include_str!("../../../../ops/config/examples/public.toml"))?;
        // 首次部署只调整回环端口；不从浏览器 Origin 推导监听地址或扩大访问范围。
        public["listener"]["address"] =
            Value::String(format!("127.0.0.1:{}", self.public_listener_port));
        public["http_security"]["allowed_origins"] =
            Value::Array(vec![Value::String(self.public_origin.clone())]);
        public["http_security"]["cookie_secure"] =
            Value::Boolean(self.public_origin.starts_with("https://"));
        public["http_security"]["trusted_proxy_cidrs"] =
            Value::Array(vec![Value::String("127.0.0.1/32".to_owned())]);
        public["realtime"]["poll_interval_millis"] =
            Value::Integer(self.realtime_poll_interval_millis as i64);
        write_toml(directory.join("public.toml"), &public, 0o640)?;

        let mut admin = template(include_str!("../../../../ops/config/examples/admin.toml"))?;
        admin["listener"]["address"] =
            Value::String(format!("127.0.0.1:{}", self.admin_listener_port));
        admin["http_security"]["allowed_origins"] =
            Value::Array(vec![Value::String(self.admin_origin.clone())]);
        admin["http_security"]["cookie_secure"] =
            Value::Boolean(self.admin_origin.starts_with("https://"));
        admin["http_security"]["trusted_proxy_cidrs"] =
            Value::Array(vec![Value::String("127.0.0.1/32".to_owned())]);
        admin["runtime_config_file"] = Value::String("runtime/admin.toml".to_owned());
        write_toml(directory.join("admin.toml"), &admin, 0o640)?;

        let mut worker = template(include_str!("../../../../ops/config/examples/worker.toml"))?;
        worker["worker"]["concurrency"] = Value::Integer(i64::from(self.worker_concurrency));
        worker["worker"]["poll_interval_seconds"] =
            Value::Integer(self.worker_poll_interval_seconds as i64);
        worker["worker"]["control"]["enabled"] = Value::Boolean(true);
        worker["worker"]["control"]["group"] = Value::String(self.control_group.clone());
        worker["worker"]["control"]["allowed_peer_uids"] =
            Value::Array(vec![Value::Integer(i64::from(self.control_uid))]);
        // 通用示例的本地联调路径不属于部署配置，防止后续启用邮件时误读主机文件。
        worker["worker"]["mooglemail"]
            .as_table_mut()
            .ok_or(FirstRunError::InvalidConfig)?
            .remove("keyenv_file");
        worker["runtime_config_file"] = Value::String("runtime/admin.toml".to_owned());
        if let Some(runtime) = managed_runtime {
            managed::configure_worker(&mut worker, runtime)?;
        }
        write_toml(directory.join("worker.toml"), &worker, 0o640)?;

        fs::DirBuilder::new()
            .mode(0o750)
            .create(directory.join("runtime"))
            .map_err(|_| FirstRunError::Io)?;
        let mut runtime =
            template(include_str!("../../../../ops/config/examples/runtime/admin.toml"))?;
        // 首次排期由公共文件拥有，不能被 runtime 模板中的候选意外接管。
        runtime["operations"]["schedule"]["managed"] = Value::Boolean(false);
        // 正式 Admin 优先读取 runtime listener，必须与角色文件的 fallback 同步。
        runtime["listener"]["address"] =
            Value::String(format!("127.0.0.1:{}", self.admin_listener_port));
        runtime["metrics"]["enabled"] = Value::Boolean(self.metrics_enabled);
        runtime["security"]["allowed_origins"] =
            Value::Array(vec![Value::String(self.admin_origin.clone())]);
        runtime["security"]["cookie_secure"] =
            Value::Boolean(self.admin_origin.starts_with("https://"));
        runtime["security"]["trusted_proxy_cidrs"] =
            Value::Array(vec![Value::String("127.0.0.1/32".to_owned())]);
        if let Some(directory) = managed_runtime {
            managed::configure_admin(&mut runtime, directory)?;
        }
        write_toml(directory.join("runtime/admin.toml"), &runtime, 0o640)?;

        let mut secrets = toml::map::Map::new();
        secrets.insert(
            "database".to_owned(),
            Value::try_from(&self.database).map_err(|_| FirstRunError::InvalidConfig)?,
        );
        secrets.insert("admin_actor".to_owned(), Value::String(self.admin_actor.clone()));
        secrets.insert("admin_secret".to_owned(), Value::String(self.admin_secret.clone()));
        write_toml(directory.join("secrets.toml"), &Value::Table(secrets), 0o600)
    }
}

/// 权限错误等无法证明目标不存在的情况失败关闭，不用布尔 exists 吞掉 I/O 错误。
fn ensure_destination_absent(destination: &Path) -> Result<(), FirstRunError> {
    match fs::symlink_metadata(destination) {
        Ok(_) => Err(FirstRunError::AlreadyExists),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(_) => Err(FirstRunError::Io),
    }
}

/// 目标名称在发布瞬间仍必须不存在，避免并发部署覆盖已出现的目录。
#[cfg(target_os = "linux")]
fn publish_directory(source: &Path, destination: &Path) -> Result<(), FirstRunError> {
    let source =
        CString::new(source.as_os_str().as_bytes()).map_err(|_| FirstRunError::InvalidInput)?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| FirstRunError::InvalidInput)?;
    // SAFETY：两个 NUL 终止路径在调用期间保持有效；内核只读取路径并原子重命名。
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        Err(FirstRunError::AlreadyExists)
    } else {
        Err(FirstRunError::Io)
    }
}

#[cfg(not(target_os = "linux"))]
fn publish_directory(_source: &Path, _destination: &Path) -> Result<(), FirstRunError> {
    Err(FirstRunError::InvalidInput)
}

fn valid_origin(value: &str) -> bool {
    let Ok(parsed) = Url::parse(value) else { return false };
    matches!(parsed.scheme(), "http" | "https")
        && (parsed.scheme() == "https"
            || matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")))
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.origin().ascii_serialization() == value
}

fn template(source: &str) -> Result<Value, FirstRunError> {
    source.parse().map_err(|_| FirstRunError::InvalidConfig)
}

fn write_toml(path: PathBuf, document: &Value, mode: u32) -> Result<(), FirstRunError> {
    let contents = toml::to_string_pretty(document).map_err(|_| FirstRunError::InvalidConfig)?;
    write_file(path, contents.as_bytes(), mode)
}

fn write_file(path: PathBuf, contents: &[u8], mode: u32) -> Result<(), FirstRunError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|_| FirstRunError::Io)?;
    file.write_all(contents).map_err(|_| FirstRunError::Io)?;
    file.sync_all().map_err(|_| FirstRunError::Io)
}
