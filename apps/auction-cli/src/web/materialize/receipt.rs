//! 页面栈回执 `<web_root>/RECEIPT.v1`（`auction-web-stack-receipt/1`）：服务启动时由启用校验
//! （`auction_config::validate_web_stack`）读取，证明寄售只在“双端 Solid、同一 build、回执登记
//! 的程序摘要等于本进程”时开启。
//!
//! 回执从当前入口指针与磁盘上的 build 推导，不接受调用方手填页面事实：入口回切到旧壳后重写的
//! 回执会如实记录 `entry_build_id` 为旧 build 与 dioxus 策略；独立入口仍是 Solid，启用校验接受“登记的旧壳”这一种回切形态，其余入口组合拒绝。包事实（包名、提交、
//! tag、外层摘要、程序摘要、服务路径与 `installed_at`）由 `install` 写入，`switch` 重写时沿用；
//! `issued_at` 是本份回执的写出时刻，每次重写都会刷新。不设挂钟过期（E9）。

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use anyhow::Context;
use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::layout::{EntryPointer, Stack, Surface};
use super::pointer::{builds_of, read_pointer};
use super::verify::BuildFacts;
use crate::support::files;

pub(super) const RECEIPT_FILE: &str = "RECEIPT.v1";
const RECEIPT_SCHEMA: &str = "auction-web-stack-receipt/1";
const WEB_STACK_ID: &str = "public-solid_admin-solid";
const MAX_RECEIPT_BYTES: u64 = 64 << 10;

/// 一个服务程序：实际服务路径与摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BinaryRecord {
    pub(super) path: String,
    pub(super) sha256: String,
}

/// 回执里与页面无关的包事实；`switch` 重写回执时从旧回执原样读回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct PackageRecord {
    pub(super) package: String,
    pub(super) source_commit: String,
    pub(super) tag: String,
    pub(super) archive_sha256: String,
    /// `auction-server` 只监管子进程、不执行寄售逻辑，登记为 `None`（JSON null）。
    pub(super) binaries: BTreeMap<String, Option<BinaryRecord>>,
    /// 该 build 首次安装的时刻：只由 `install` 写入，`switch` 重写回执时原样沿用、不得刷新；
    /// 冷回切演练回执以它为早于实物的下界（`issued_at` 每次重写都会变）。
    pub(super) installed_at: Timestamp,
}

/// 两端的页面事实：当前入口指针与唯一的旧壳 build。
struct SurfaceRecord {
    pointer: EntryPointer,
    legacy: BuildFacts,
}

/// 即将换入、尚未写盘的入口指针：用它替换对应 surface 磁盘上的当前指针来推导回执，
/// 使 `switch` 能按“先拒绝、后放行”的顺序写盘。
pub(super) type PendingPointer<'a> = Option<(Surface, &'a EntryPointer)>;

/// 从两端的入口指针与旧壳 build 推导回执；两端 Solid build 必须是同一个 `build_id`。
pub(super) fn render(
    package: &PackageRecord,
    web_root: &Path,
    pending: PendingPointer<'_>,
    issued_at: Timestamp,
) -> anyhow::Result<Value> {
    let public = surface_record(web_root, Surface::Public, pending)?;
    let admin = surface_record(web_root, Surface::Admin, pending)?;
    if public.pointer.independent_build_id != admin.pointer.independent_build_id {
        anyhow::bail!("Public 与 Admin 的 Solid build 不是同一个 build_id，不写回执");
    }
    Ok(json!({
        "schema": RECEIPT_SCHEMA,
        "package": package.package,
        "source_commit": package.source_commit,
        "tag": package.tag,
        "archive_sha256": package.archive_sha256,
        "web_stack_id": WEB_STACK_ID,
        "build_id": public.pointer.independent_build_id,
        "surfaces": { "public": surface_json(&public), "admin": surface_json(&admin) },
        "binaries": package.binaries,
        "installed_at": package.installed_at.to_rfc3339(),
        "issued_at": issued_at.to_rfc3339(),
    }))
}

fn surface_json(record: &SurfaceRecord) -> Value {
    json!({
        "stack": Stack::Solid.as_str(),
        "payload_sha256": record.pointer.independent_payload_sha256,
        "entry_build_id": record.pointer.build_id,
        "entry_script_policy": record.pointer.stack.as_str(),
        "legacy": {
            "build_id": record.legacy.manifest.build_id,
            "payload_sha256": record.legacy.manifest.payload_sha256,
            "script_policy": record.legacy.manifest.stack.as_str(),
        },
    })
}

fn surface_record(
    web_root: &Path,
    surface: Surface,
    pending: PendingPointer<'_>,
) -> anyhow::Result<SurfaceRecord> {
    let surface_root = web_root.join(surface.as_str());
    let pointer = match pending {
        Some((target, pointer)) if target == surface => pointer.clone(),
        _ => read_pointer(&surface_root, surface)?
            .with_context(|| format!("{} 没有入口指针", surface.as_str()))?,
    };
    let mut legacy: Vec<BuildFacts> = builds_of(&surface_root, surface)?
        .into_iter()
        .filter(|facts| facts.manifest.stack == Stack::Dioxus)
        .collect();
    if legacy.len() != 1 {
        anyhow::bail!(
            "{} 需要恰好一个已物化的旧壳（dioxus）build，实际 {} 个",
            surface.as_str(),
            legacy.len()
        );
    }
    let legacy = legacy.remove(0);
    Ok(SurfaceRecord { pointer, legacy })
}

/// 同目录临时文件 + fsync + rename：服务读到的要么是旧回执，要么是完整的新回执。
pub(super) fn write(web_root: &Path, receipt: &Value) -> anyhow::Result<()> {
    let temporary = web_root.join(format!(".{RECEIPT_FILE}.tmp-{}", Uuid::new_v4()));
    let mut text = serde_json::to_string_pretty(receipt).context("序列化页面栈回执失败")?;
    text.push('\n');
    let result = (|| -> anyhow::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&temporary)
            .context("创建回执临时文件失败")?;
        file.write_all(text.as_bytes()).context("写入回执失败")?;
        file.sync_all().context("同步回执失败")?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644))
            .context("设置回执权限失败")?;
        fs::rename(&temporary, web_root.join(RECEIPT_FILE)).context("替换回执失败")
    })();
    if let Err(error) = result {
        return match fs::remove_file(&temporary) {
            Ok(()) => Err(error),
            Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => Err(error),
            Err(cleanup) => Err(error.context(format!("且清理临时文件失败：{cleanup}"))),
        };
    }
    files::sync_directory(web_root, "挂载根目录")
}

/// 读回已有回执里的包事实；没有回执返回 `None`，存在但无法解析则失败（不静默重建）。
pub(super) fn read_package(web_root: &Path) -> anyhow::Result<Option<PackageRecord>> {
    let path = web_root.join(RECEIPT_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("读取回执失败：{}", path.display()));
        }
    }
    let bytes = files::read_bounded(&path, MAX_RECEIPT_BYTES)?;
    let package = serde_json::from_slice(&bytes).context("已有的页面栈回执无法解析")?;
    Ok(Some(package))
}
