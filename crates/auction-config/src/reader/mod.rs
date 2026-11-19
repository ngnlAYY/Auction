//! 角色、公共和普通敏感 TOML 的有界读取；文件类型检查与读取使用同一描述符。

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::{
    fs::OpenOptions,
    io::{self, Read},
    path::Path,
};

const MAX_CONFIG_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// 保留部署路径的 symlink 兼容性，但拒绝特殊文件及超过 4 MiB 的输入。
/// runtime 与维护凭据另有更严格的权限校验，不能由此入口绕过。
pub(crate) fn read_toml_file(path: &Path) -> io::Result<String> {
    String::from_utf8(read_toml_bytes(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.utf8_error()))
}

/// 保留原始字节，使部署文件可以在解码前核对指纹。
pub(crate) fn read_toml_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    // FIFO 必须先以非阻塞方式打开，才能进入同一描述符上的普通文件检查。
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "TOML 配置必须是普通文件"));
    }
    if metadata.len() > MAX_CONFIG_FILE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "TOML 配置不能超过 4 MiB"));
    }
    let mut contents = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CONFIG_FILE_BYTES + 1).read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_CONFIG_FILE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "TOML 配置不能超过 4 MiB"));
    }
    Ok(contents)
}
