//! 装配根共用的寄售启用判定：四事实之后读取挂载根下的页面栈回执并核对本进程身份（D9 §8、E9）。
//!
//! 四事实判定为关闭或拒绝时不读任何文件；开启时才读回执与本程序文件。回执缺失、过大、无法读取，
//! 或本程序摘要无法取得，一律失败关闭。错误只含固定原因，不回显回执内容或路径。
//! 唯一例外是本地调试预览（编译期同时有 `consignment-dev-preview` 与 `debug_assertions`），见
//! [`process_consignment_activation`]。

use std::fs::File;
use std::io::{ErrorKind, Read};

use sha2::{Digest, Sha256};

use crate::settings::AppConfig;
use crate::settings::consignment::web_stack::{WebStackBinary, WebStackFacts, validate_web_stack};
use crate::settings::consignment::{
    CONSIGNMENT_COMPLETE, ConsignmentActivation, consignment_activation,
};
use crate::settings::errors::ConfigError;

/// 回执是小 JSON；超过上限视为损坏，不读入内存。
const RECEIPT_MAX_BYTES: u64 = 1 << 20;

/// 不变量 6：寄售只在制品门与页面栈回执都通过时开启；`preview`、`debug` 由调用方的编译期事实给出，
/// `own_build_id` 由装配根从唯一 owner `auction_observability::process_build_id` 取得。
///
/// 本地调试预览（`preview` 与 `debug` 同时为真）开启时不要求回执：这两项都不可能出现在发布制品里
/// （正式与候选构建禁 preview，且不是调试构型），例外不削弱生产门；其余组合缺回执一律失败关闭。
pub fn process_consignment_activation(
    config: &AppConfig,
    binary: WebStackBinary,
    own_build_id: &str,
    preview: bool,
    debug: bool,
) -> Result<ConsignmentActivation, ConfigError> {
    activation_for(config, binary, own_build_id, CONSIGNMENT_COMPLETE, preview, debug)
}

fn activation_for(
    config: &AppConfig,
    binary: WebStackBinary,
    own_build_id: &str,
    complete: bool,
    preview: bool,
    debug: bool,
) -> Result<ConsignmentActivation, ConfigError> {
    let activation =
        consignment_activation(config.consignment.enabled(), complete, preview, debug)?;
    if activation == ConsignmentActivation::Disabled || (preview && debug) {
        return Ok(activation);
    }
    let receipt = config.web.receipt_path().map(|path| read_receipt(&path)).transpose()?.flatten();
    let own_binary_sha256 = own_binary_sha256();
    validate_web_stack(&WebStackFacts {
        receipt_bytes: receipt.as_deref(),
        binary,
        own_binary_sha256: own_binary_sha256.as_deref(),
        own_build_id,
    })?;
    Ok(activation)
}

/// 回执不存在返回 `None`，由校验报“缺少回执”；其他读取失败与超限直接拒绝。
fn read_receipt(path: &std::path::Path) -> Result<Option<Vec<u8>>, ConfigError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_error) => return Err(ConfigError::ConsignmentWebStack("无法读取 Web 栈回执")),
    };
    let mut bytes = Vec::new();
    file.take(RECEIPT_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_error| ConfigError::ConsignmentWebStack("无法读取 Web 栈回执"))?;
    if bytes.len() as u64 > RECEIPT_MAX_BYTES {
        return Err(ConfigError::ConsignmentWebStack("Web 栈回执超过大小上限"));
    }
    Ok(Some(bytes))
}

/// 以 `/proc/self/exe` 计算正在运行的程序文件摘要；部署替换文件后旧进程仍指向原 inode。
fn own_binary_sha256() -> Option<String> {
    let mut file = File::open("/proc/self/exe").ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

/// 测试入口：显式给出 `complete`，使发布构型（非预览）的回执路径在功能完成前也可验证。
#[cfg(test)]
pub(super) fn test_activation(
    config: &AppConfig,
    binary: WebStackBinary,
    own_build_id: &str,
    complete: bool,
    preview: bool,
    debug: bool,
) -> Result<ConsignmentActivation, ConfigError> {
    activation_for(config, binary, own_build_id, complete, preview, debug)
}

#[cfg(test)]
pub(super) fn test_own_binary_sha256() -> Option<String> {
    own_binary_sha256()
}
