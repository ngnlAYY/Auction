//! 冷回切演练回执核对：`auction-cli web drill-verify` 的实现。
//!
//! 演练回执 `auction-web-switch-drill.v1` 记录 D9 第 8 节步骤②–⑤ 的逐响应观测（新版、回切旧壳、
//! 再切新版三个阶段，两端各自）与浏览器步骤的人工签收。核对分两层：回执必须来自本次最终 tag 实物
//! （tag、build ID、外层摘要都绑定 `<web_root>/RECEIPT.v1`，实物时间取其 `installed_at`（首次安装时刻，`switch` 重写不刷新），调用方不能
//! 另行指定）且字节未被改写（审核人带外给出回执文件摘要）；每个响应的构建号、入口 HTML 摘要与策略必须与它所属的 build 一致，
//! 错配（旧 HTML 配 Solid 策略、新版页面配旧策略、304 复用错版本策略）一律拒绝，即使状态码与摘要都对。
//! 不设挂钟过期：回执“过期”指早于最终 tag 实物（E9 与 R7 §2.3 的 8C-S 一行）。

use std::path::Path;

use anyhow::Context;
use auction_time::Timestamp;
use serde::Deserialize;
use serde_json::json;

use crate::app::print_json;
use crate::support::{digest, files};

mod validation;

pub(crate) const FORMAT: &str = "auction-web-switch-drill.v1";
const MAX_RECEIPT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_WEB_RECEIPT_BYTES: u64 = 64 << 10;
const RECEIPT_FILE: &str = "RECEIPT.v1";

/// 本次最终 tag 实物的事实；演练回执必须逐项一致。
pub(crate) struct Requirements {
    /// 审核人在签收时对回执文件计算的 SHA-256（带外给出）；不符即视为被改写。
    pub(crate) receipt_sha256: String,
    pub(crate) tag: String,
    pub(crate) source_commit: String,
    pub(crate) archive_sha256: String,
    pub(crate) build_id: String,
    /// 部署回执 `RECEIPT.v1` 的 `installed_at`（该 build 首次安装的时刻）；演练必须在它之后开始，
    /// 否则演练的不是这份实物。
    pub(crate) artifact_created_at: Timestamp,
    pub(crate) now: Timestamp,
}

/// 通过核对的最小证据。
#[derive(Debug)]
pub(crate) struct DrillEvidence {
    pub(crate) drill_id: String,
    pub(crate) tag: String,
    pub(crate) build_id: String,
    pub(crate) responses: usize,
}

/// `<web_root>/RECEIPT.v1` 里与实物绑定有关的字段；其余字段由启用校验读取，这里忽略。
#[derive(Deserialize)]
struct WebStackReceiptFacts {
    tag: String,
    source_commit: String,
    archive_sha256: String,
    build_id: String,
    installed_at: Timestamp,
}

/// 从部署回执取最终实物的身份；回执缺失或无法解析即失败，不退回调用方手填。
pub(crate) fn requirements_from_web_root(
    web_root: &Path,
    receipt_sha256: &str,
    now: Timestamp,
) -> anyhow::Result<Requirements> {
    let bytes = files::read_bounded(&web_root.join(RECEIPT_FILE), MAX_WEB_RECEIPT_BYTES)
        .context("读取页面栈回执失败")?;
    let facts: WebStackReceiptFacts =
        serde_json::from_slice(&bytes).context("解析页面栈回执失败")?;
    Ok(Requirements {
        receipt_sha256: receipt_sha256.to_owned(),
        tag: facts.tag,
        source_commit: facts.source_commit,
        archive_sha256: facts.archive_sha256,
        build_id: facts.build_id,
        artifact_created_at: facts.installed_at,
        now,
    })
}

/// 读取并核对演练回执；摘要在解析之前先核，被改写的文件不进入后续逻辑。
pub(crate) fn inspect(path: &Path, requirements: &Requirements) -> anyhow::Result<DrillEvidence> {
    if !path.is_absolute() {
        anyhow::bail!("--receipt 必须是绝对路径");
    }
    let bytes = files::read_bounded(path, MAX_RECEIPT_BYTES)
        .map_err(|error| anyhow::anyhow!("读取页面切换演练回执失败：{error}"))?;
    if digest::sha256_bytes(&bytes) != requirements.receipt_sha256 {
        anyhow::bail!("页面切换演练回执的摘要与审核人签收时的摘要不一致，回执已被改写");
    }
    let receipt: validation::SwitchDrillReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("解析页面切换演练回执失败：{error}"))?;
    receipt.validate(requirements)
}

/// `web drill-verify` 入口：核对并输出 JSON 结果。
pub(crate) fn run(receipt: &Path, receipt_sha256: &str, web_root: &Path) -> anyhow::Result<()> {
    if !digest::is_lower_hex(receipt_sha256, 64) {
        anyhow::bail!("--receipt-sha256 必须是 64 位小写十六进制");
    }
    let requirements = requirements_from_web_root(web_root, receipt_sha256, Timestamp::now())?;
    let evidence = inspect(receipt, &requirements)?;
    print_json(&json!({
        "drill_id": evidence.drill_id,
        "tag": evidence.tag,
        "build_id": evidence.build_id,
        "responses": evidence.responses,
        "verified": true,
    }))
}

#[cfg(test)]
mod tests;
