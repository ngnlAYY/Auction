//! 来源验证回执 `auction-package-verification/1`：由 `ci/release/verify-deployment-package.sh`
//! 校验 GitHub 来源证明后写出；CLI 不调用 `gh`，只核对回执与外层摘要、解压目录清单的绑定。
//! 回执是本机文件，只防部署人员误装，不防本机恶意（见 R7 E4）。

use anyhow::Context;
use auction_time::{Duration, Timestamp};

use super::{Artifact, WebStackIdentity, web_stack};
use crate::support::digest::is_lower_hex;

const SCHEMA: &str = "auction-package-verification/1";
/// 回执字段闭集，顺序固定。
const FIELDS: [&str; 8] = [
    "schema",
    "archive_sha256",
    "package",
    "source_commit",
    "tag",
    "provenance",
    "web_stack_id",
    "verified_at",
];
/// 回执的最大年龄；超龄说明验证与部署之间包可能被替换过。
const MAX_AGE: Duration = Duration::hours(24);
/// 允许的时钟前移：回执时刻晚于当前时刻超过它即拒绝。
const MAX_FUTURE_SKEW: Duration = Duration::minutes(5);
const MAX_RECEIPT_BYTES: usize = 4096;

/// 来源证明的结果；`--offline` 只会写出 `Skipped`，不能作为部署依据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provenance {
    Verified,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageVerification {
    pub(crate) archive_sha256: String,
    pub(crate) package: String,
    pub(crate) source_commit: String,
    pub(crate) tag: String,
    pub(crate) provenance: Provenance,
    pub(crate) web_stack_id: String,
    pub(crate) verified_at: Timestamp,
}

/// 解析回执：恰为八行、字段按固定顺序、LF 结尾，值各自合乎形状；任何偏离都拒绝。
pub(crate) fn parse(bytes: &[u8]) -> anyhow::Result<PackageVerification> {
    if bytes.len() > MAX_RECEIPT_BYTES {
        anyhow::bail!("验证回执超过 {MAX_RECEIPT_BYTES} 字节");
    }
    let text = std::str::from_utf8(bytes).context("验证回执不是 UTF-8")?;
    let body = text.strip_suffix('\n').context("验证回执必须以 LF 结尾")?;
    let lines: Vec<&str> = body.split('\n').collect();
    if lines.len() != FIELDS.len() {
        anyhow::bail!("验证回执必须恰为 {} 行", FIELDS.len());
    }
    let mut values = Vec::with_capacity(FIELDS.len());
    for (index, (line, key)) in lines.iter().zip(FIELDS).enumerate() {
        let value = line
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
            .with_context(|| format!("验证回执第 {} 行应为 {key}", index + 1))?;
        values.push(value);
    }
    let [schema, archive, package, commit, tag, provenance, web_stack_id, verified_at] = values[..]
    else {
        anyhow::bail!("验证回执字段数量不符");
    };
    if schema != SCHEMA {
        anyhow::bail!("验证回执的 schema 不受支持");
    }
    if !is_lower_hex(archive, 64) || !is_lower_hex(commit, 40) {
        anyhow::bail!("验证回执的摘要或提交无效");
    }
    let provenance = match provenance {
        "verified" => Provenance::Verified,
        "skipped" => Provenance::Skipped,
        _ => anyhow::bail!("验证回执的 provenance 无效"),
    };
    if !matches!(web_stack_id, "public-solid_admin-solid" | "none") {
        anyhow::bail!("验证回执的 web_stack_id 无效");
    }
    let verified_at = Timestamp::parse_from_rfc3339(verified_at)
        .map_err(|error| anyhow::anyhow!("验证回执的 verified_at 无效：{error}"))?;
    Ok(PackageVerification {
        archive_sha256: archive.to_owned(),
        package: package.to_owned(),
        source_commit: commit.to_owned(),
        tag: tag.to_owned(),
        provenance,
        web_stack_id: web_stack_id.to_owned(),
        verified_at,
    })
}

/// 核对回执与已检查的制品、外层摘要的绑定。
///
/// 新格式包（带 `WEB-STACK.v1`）缺回执即拒；旧格式包回执可缺，给出则同样必须全部对上。
/// 外层摘要由调用方传入（`--package-sha256`），回执里的摘要必须与之相等，于是回执证明的
/// 就是正在部署的那个压缩包。
pub(crate) fn check(
    artifact: &Artifact,
    receipt: Option<&PackageVerification>,
    package_sha256: &str,
    now: Timestamp,
) -> anyhow::Result<()> {
    let Some(receipt) = receipt else {
        if artifact.web_stack.is_some() {
            anyhow::bail!("新格式发布包必须提供来源验证回执");
        }
        return Ok(());
    };
    if receipt.archive_sha256 != package_sha256 {
        anyhow::bail!("验证回执的外层摘要与 --package-sha256 不一致");
    }
    if receipt.package != artifact.package || receipt.source_commit != artifact.source_commit {
        anyhow::bail!("验证回执的包名或提交与解压目录清单不一致");
    }
    let version = web_stack::package_version(&artifact.package)?;
    if receipt.tag != format!("v{version}") {
        anyhow::bail!("验证回执的 tag 与包版本不一致");
    }
    if receipt.provenance != Provenance::Verified {
        anyhow::bail!("验证回执未经过来源证明（--offline 的回执不能作为部署依据）");
    }
    let expected_stack =
        artifact.web_stack.as_ref().map_or_else(|| "none".to_owned(), WebStackIdentity::id);
    if receipt.web_stack_id != expected_stack {
        anyhow::bail!("验证回执的 web_stack_id 与制品页面凭据不一致");
    }
    let age = now.signed_duration_since(receipt.verified_at);
    if age > MAX_AGE || age < Duration::zero() - MAX_FUTURE_SKEW {
        anyhow::bail!("验证回执已过期或时刻在未来，请重新验证");
    }
    Ok(())
}
