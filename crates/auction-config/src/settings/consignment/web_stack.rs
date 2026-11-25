//! 寄售启用前的 Web 栈回执校验：纯函数，不做 IO，由装配根读文件后传入字节。
//!
//! 保护的不变量：寄售只在“双端独立入口是本包 Solid build、回执登记的程序摘要等于本进程”时开启；
//! 默认入口只能是本包 Solid，或回执登记的旧壳（回切期间，独立订单、权利与回执入口仍在 Solid）。
//! 缺回执、单端、混包、build 或程序摘要不符、其他入口组合一律失败关闭。回执不设挂钟过期，
//! 被后续部署取代即程序摘要不再匹配。

use serde::Deserialize;

use crate::settings::auction::Role;
use crate::settings::consignment::{ConsignmentActivation, consignment_activation};
use crate::settings::errors::ConfigError;

const RECEIPT_SCHEMA: &str = "auction-web-stack-receipt/1";
const WEB_STACK_ID: &str = "public-solid_admin-solid";

/// 回执 `binaries` 中登记摘要的程序：三个常驻角色、执行 `config check` / `keys` 的 CLI（E9 补充）
/// 与合并部署的 `auction-server`（D9 §8：各装配根共用同一校验器）。
///
/// 不并入 [`Role`]：`Role` 是可加载配置的角色闭集，CLI 与 server 不以自身角色加载配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebStackBinary {
    /// `auction-public-api`。
    Public,
    /// `auction-admin-api`。
    Admin,
    /// `auction-worker`。
    Worker,
    /// `auction-cli`。
    Cli,
    /// `auction-server`；回执目前登记为 `null`，合并部署开启寄售因此失败关闭。
    Server,
}

impl From<Role> for WebStackBinary {
    fn from(role: Role) -> Self {
        match role {
            Role::Public => Self::Public,
            Role::Admin => Self::Admin,
            Role::Worker => Self::Worker,
        }
    }
}

/// 启用校验需要的进程事实，回执字节由调用方读取。
#[derive(Debug, Clone, Copy)]
pub struct WebStackFacts<'a> {
    /// 回执文件的原始字节；未配置或未读到时为 `None`。
    pub receipt_bytes: Option<&'a [u8]>,
    /// 当前程序，用于在回执的 `binaries` 中选取自身条目。
    pub binary: WebStackBinary,
    /// 当前可执行文件的小写十六进制 SHA-256。
    pub own_binary_sha256: Option<&'a str>,
    /// 当前进程所属的 build ID。
    pub own_build_id: &'a str,
}

#[derive(Deserialize)]
struct Receipt {
    schema: String,
    web_stack_id: String,
    build_id: String,
    surfaces: Surfaces,
    binaries: Binaries,
}

#[derive(Deserialize)]
struct Surfaces {
    public: Option<Surface>,
    admin: Option<Surface>,
}

#[derive(Deserialize)]
struct Surface {
    stack: String,
    payload_sha256: String,
    entry_build_id: String,
    entry_script_policy: String,
    legacy: Option<Legacy>,
}

#[derive(Deserialize)]
struct Legacy {
    build_id: String,
    payload_sha256: String,
    script_policy: String,
}

#[derive(Deserialize)]
struct Binaries {
    #[serde(rename = "auction-public-api")]
    public: Option<Binary>,
    #[serde(rename = "auction-admin-api")]
    admin: Option<Binary>,
    #[serde(rename = "auction-worker")]
    worker: Option<Binary>,
    #[serde(rename = "auction-cli")]
    cli: Option<Binary>,
    #[serde(rename = "auction-server")]
    server: Option<Binary>,
}

#[derive(Deserialize)]
struct Binary {
    sha256: String,
}

fn rejected(reason: &'static str) -> ConfigError {
    ConfigError::ConsignmentWebStack(reason)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn validate_surface(surface: Option<&Surface>, build_id: &str) -> Result<(), ConfigError> {
    let surface = surface.ok_or_else(|| rejected("回执缺少某一端的页面栈（单端）"))?;
    // 独立入口：始终是本包 Solid build（顶层 build_id 与本程序的比对在调用方）。
    if surface.stack != "solid" {
        return Err(rejected("回执中某一端的独立入口不是 Solid 栈"));
    }
    if !is_sha256_hex(&surface.payload_sha256) {
        return Err(rejected("回执页面负载摘要不是 64 位小写十六进制"));
    }
    let legacy = match &surface.legacy {
        Some(legacy) if legacy.script_policy == "dioxus" => legacy,
        _ => return Err(rejected("回执缺少旧壳（dioxus）回切记录")),
    };
    if legacy.build_id == build_id || !is_sha256_hex(&legacy.payload_sha256) {
        return Err(rejected("回执的旧壳记录与本包 Solid build 相同或摘要无效"));
    }
    // 默认入口二选一：本包 Solid，或登记的旧壳；其余组合（含未登记 build、build 与策略错配）拒绝。
    let on_solid = surface.entry_build_id == build_id && surface.entry_script_policy == "solid";
    let on_legacy =
        surface.entry_build_id == legacy.build_id && surface.entry_script_policy == "dioxus";
    if on_solid || on_legacy {
        Ok(())
    } else {
        Err(rejected("回执默认入口既不是本包 Solid，也不是登记的旧壳（混包或错配）"))
    }
}

/// 校验回执是否足以开启寄售：双端独立入口是本包 Solid、默认入口在本包 Solid 或登记的旧壳、程序摘要一致。
pub fn validate_web_stack(facts: &WebStackFacts<'_>) -> Result<(), ConfigError> {
    let bytes = facts.receipt_bytes.ok_or_else(|| rejected("缺少 Web 栈回执"))?;
    let receipt: Receipt =
        serde_json::from_slice(bytes).map_err(|_| rejected("Web 栈回执无法解析"))?;
    if receipt.schema != RECEIPT_SCHEMA {
        return Err(rejected("Web 栈回执的 schema 不受支持"));
    }
    if receipt.web_stack_id != WEB_STACK_ID {
        return Err(rejected("回执不是双端 Solid 的同一部署（栈标识不符）"));
    }
    validate_surface(receipt.surfaces.public.as_ref(), &receipt.build_id)?;
    validate_surface(receipt.surfaces.admin.as_ref(), &receipt.build_id)?;
    if receipt.build_id != facts.own_build_id {
        return Err(rejected("回执 build ID 与当前程序的 build ID 不一致"));
    }
    let registered = match facts.binary {
        WebStackBinary::Public => receipt.binaries.public,
        WebStackBinary::Admin => receipt.binaries.admin,
        WebStackBinary::Worker => receipt.binaries.worker,
        WebStackBinary::Cli => receipt.binaries.cli,
        WebStackBinary::Server => receipt.binaries.server,
    }
    .ok_or_else(|| rejected("回执没有登记当前角色的程序摘要"))?;
    let own = facts.own_binary_sha256.ok_or_else(|| rejected("无法取得当前程序的摘要"))?;
    if !is_sha256_hex(own) || registered.sha256 != own {
        return Err(rejected("回执登记的程序摘要与当前程序不一致"));
    }
    Ok(())
}

/// 在四事实判定之后追加回执校验；`enabled=false` 时不读回执、不报错。
pub fn consignment_activation_with_web_stack(
    enabled: bool,
    complete: bool,
    preview: bool,
    debug: bool,
    web_stack: &WebStackFacts<'_>,
) -> Result<ConsignmentActivation, ConfigError> {
    let activation = consignment_activation(enabled, complete, preview, debug)?;
    if activation == ConsignmentActivation::Enabled {
        validate_web_stack(web_stack)?;
    }
    Ok(activation)
}
