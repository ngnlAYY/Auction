//! 本地 `.keyenv` 的 MoogleMail 双身份解析。

use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use super::super::ConfigError;
use super::model::{MoogleMailKeyEnv, SecretString};

/// 读取本地 `.keyenv` 的两个 MoogleMail Cookie。
/// 这是用户明确提供的本地开发/联调凭据通道，不是通用环境变量覆盖机制。解析器只接受
/// `hv-ck-key1` 和 `hv-ck-key2`，拒绝未知键、重复键、空值和不安全文件权限；错误不会回显
/// 任何值。
pub fn load_mooglemail_keyenv(path: impl AsRef<Path>) -> Result<MoogleMailKeyEnv, ConfigError> {
    const MAX_KEYENV_BYTES: u64 = 64 * 1024;
    let path = path.as_ref().to_owned();
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options
        .open(&path)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    // 权限与内容必须来自同一描述符；路径替换不能绕过已完成的安全检查。
    let metadata = file
        .metadata()
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    if !metadata.is_file() {
        return Err(ConfigError::MoogleMailKeyEnvRead {
            path,
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not a regular file"),
        });
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(ConfigError::MoogleMailKeyEnvInsecurePermissions { path });
    }

    if metadata.len() > MAX_KEYENV_BYTES {
        return Err(ConfigError::MoogleMailKeyEnvParse { path });
    }
    let mut contents = String::with_capacity(metadata.len() as usize);
    file.take(MAX_KEYENV_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|source| ConfigError::MoogleMailKeyEnvRead { path: path.clone(), source })?;
    if contents.len() as u64 > MAX_KEYENV_BYTES {
        return Err(ConfigError::MoogleMailKeyEnvParse { path });
    }
    let mut bot_cookie = None;
    let mut test_cookie = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        let key = raw_key.trim();
        let Some(value) = parse_mooglemail_keyenv_value(raw_value) else {
            return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() });
        };
        match key {
            "hv-ck-key1" if bot_cookie.is_none() => bot_cookie = Some(value),
            "hv-ck-key2" if test_cookie.is_none() => test_cookie = Some(value),
            _ => return Err(ConfigError::MoogleMailKeyEnvParse { path: path.clone() }),
        }
    }
    let bot_cookie = bot_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key1",
    })?;
    let test_cookie = test_cookie.ok_or_else(|| ConfigError::MoogleMailKeyEnvMissing {
        path: path.clone(),
        key: "hv-ck-key2",
    })?;
    Ok(MoogleMailKeyEnv {
        bot_cookie: SecretString::new("hv-ck-key1", bot_cookie)?,
        test_cookie: SecretString::new("hv-ck-key2", test_cookie)?,
    })
}

fn parse_mooglemail_keyenv_value(raw_value: &str) -> Option<String> {
    let value = if let Some(value) = raw_value.strip_prefix('"') {
        let value = value.strip_suffix('"')?;
        if value.contains(['"', '\\']) {
            return None;
        }
        value
    } else if let Some(value) = raw_value.strip_prefix('\'') {
        let value = value.strip_suffix('\'')?;
        if value.contains(['\'', '\\']) {
            return None;
        }
        value
    } else {
        if raw_value.contains(['"', '\'']) {
            return None;
        }
        raw_value
    };
    auction_domain::valid_text(value, 4096).then(|| value.to_owned())
}
