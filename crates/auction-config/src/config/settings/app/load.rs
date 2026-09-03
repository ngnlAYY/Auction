//! 应用配置文件定位、读取和输入模型装配。

use super::super::*;
use super::model::{AppConfig, BuildInput, CommonFileConfig, FileConfig};

impl AppConfig {
    pub fn load(role: Role) -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_FILE_ENV).map(PathBuf::from) {
            return Self::load_from_path(role, path);
        }

        let default_path = PathBuf::from("ops/config").join(format!("{}.toml", role.name()));
        if default_path.is_file() {
            return Self::load_from_path(role, default_path);
        }

        Self::build(
            role,
            BuildInput {
                config_path: None,
                common_config_path: None,
                listen_address: None,
                listener: None,
                log_filter: None,
                metrics: MetricsConfig::default(),
                tracing: TracingSettings::default(),
                realtime: RealtimeConfig::default(),
                cache: CacheConfig::default(),
                auction: AuctionConfig::default(),
                worker: WorkerConfig::default(),
                http_security: None,
                session: None,
                secrets_path: None,
                runtime_config_path: None,
            },
        )
    }

    pub fn load_from_path(role: Role, path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_owned();
        let contents = fs::read_to_string(&path)
            .map_err(|source| ConfigError::Read { path: path.clone(), source })?;
        let file: FileConfig =
            toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.clone() })?;

        Self::from_file(role, Some(path), file)
    }

    fn from_file(
        role: Role,
        config_path: Option<PathBuf>,
        file: FileConfig,
    ) -> Result<Self, ConfigError> {
        let FileConfig {
            common_file,
            listen_address,
            listener,
            tracing,
            realtime,
            secrets_file,
            runtime_config_file,
            worker,
            http_security,
            session,
        } = file;
        let common_config_path = resolve_config_path(config_path.as_deref(), common_file);
        let common = load_common_file(&common_config_path)?;
        let CommonFileConfig { log_filter, metrics, cache, auction } = common;
        let worker = resolve_worker_paths(config_path.as_deref(), worker);
        let secrets_path = Some(resolve_secrets_path(config_path.as_deref(), secrets_file));
        let runtime_config_path =
            runtime_config_file.map(|path| resolve_config_path(config_path.as_deref(), path));

        Self::build(
            role,
            BuildInput {
                config_path,
                common_config_path: Some(common_config_path),
                listen_address,
                listener,
                log_filter,
                metrics,
                tracing,
                realtime,
                cache,
                auction,
                worker,
                http_security,
                session,
                secrets_path,
                runtime_config_path,
            },
        )
    }
}

/// 公共文件使用独立闭集模型；解析错误只报告路径，避免旧文件中的误放敏感值被回显。
fn load_common_file(path: &Path) -> Result<CommonFileConfig, ConfigError> {
    let contents = fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.to_owned(), source })?;
    toml::from_str(&contents).map_err(|_| ConfigError::Parse { path: path.to_owned() })
}
