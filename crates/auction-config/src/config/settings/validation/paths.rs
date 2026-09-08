//! 配置文件、敏感文件、Worker keyenv 和数据库 URL 路径辅助。

use super::super::*;

pub(crate) fn resolve_secrets_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

pub(crate) fn resolve_config_path(config_path: Option<&Path>, requested_path: PathBuf) -> PathBuf {
    if requested_path.is_absolute() {
        return requested_path;
    }

    config_path.and_then(Path::parent).unwrap_or_else(|| Path::new(".")).join(requested_path)
}

pub(crate) fn resolve_worker_paths(
    config_path: Option<&Path>,
    mut worker: WorkerConfig,
) -> WorkerConfig {
    worker.mooglemail.keyenv_file =
        worker.mooglemail.keyenv_file.map(|path| resolve_config_path(config_path, path));
    worker.mooglemail.diagnostics.directory =
        resolve_config_path(config_path, worker.mooglemail.diagnostics.directory);
    worker
}

pub(crate) fn database_url_is_sqlite(value: &str) -> bool {
    value.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("sqlite"))
}
