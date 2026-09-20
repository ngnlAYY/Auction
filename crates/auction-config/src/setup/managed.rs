//! 合并部署的确定性运行目录布局，仍复用三角色 TOML。

use std::path::{Path, PathBuf};

use super::{FirstRunError, Value};

/// 从绝对配置目录推导同级私有运行目录；它不属于待原子发布的配置目录。
pub fn managed_runtime_directory(config_dir: &Path) -> Result<PathBuf, FirstRunError> {
    if !config_dir.is_absolute()
        || config_dir.components().any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(FirstRunError::InvalidInput);
    }
    let parent = config_dir.parent().ok_or(FirstRunError::InvalidInput)?;
    let name =
        config_dir.file_name().and_then(|name| name.to_str()).ok_or(FirstRunError::InvalidInput)?;
    Ok(parent.join(format!(".{name}.server")))
}

pub(super) fn configure_worker(document: &mut Value, runtime: &Path) -> Result<(), FirstRunError> {
    let directory = runtime.to_str().ok_or(FirstRunError::InvalidInput)?;
    let socket =
        runtime.join("control.sock").to_str().ok_or(FirstRunError::InvalidInput)?.to_owned();
    document["worker"]["control"]["runtime_directory"] = Value::String(directory.to_owned());
    document["worker"]["control"]["socket_path"] = Value::String(socket);
    Ok(())
}

pub(super) fn configure_admin(document: &mut Value, runtime: &Path) -> Result<(), FirstRunError> {
    let socket =
        runtime.join("control.sock").to_str().ok_or(FirstRunError::InvalidInput)?.to_owned();
    document["worker_control"]["socket_path"] = Value::String(socket);
    Ok(())
}
