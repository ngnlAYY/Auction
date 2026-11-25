//! 新版发布包的来源核对：制品静态校验 + 来源验证回执与外层摘要、解压目录清单的绑定。
//!
//! 来源证明（`gh attestation verify`）只在 `ci/release/verify-deployment-package.sh` 里做，CLI 不调用
//! `gh`，只核对该脚本写出的回执（R7 E4）；回执是本机文件，只防部署人员误装，不防本机恶意。

use std::path::Path;

use anyhow::Context;
use auction_time::Timestamp;

use crate::cli::VerifyArgs;
use crate::deploy::artifact::{self, Artifact, package_receipt};
use crate::support::files;

/// 回执文件的读取上限；回执恰为八行短文本，远小于此值。
const MAX_RECEIPT_FILE_BYTES: u64 = 4096;

/// 一个通过核对的新版包及其验证回执中的事实。
pub(super) struct VerifiedPackage {
    pub(super) artifact: Artifact,
    pub(super) tag: String,
    pub(super) archive_sha256: String,
    pub(super) verified_at: Timestamp,
}

/// 校验制品、读取回执并核对绑定；新格式包没有页面凭据或回执缺失都失败。
pub(super) fn verify_package(args: &VerifyArgs, now: Timestamp) -> anyhow::Result<VerifiedPackage> {
    let artifact = artifact::inspect(&args.artifact_dir).context("发布包校验失败")?;
    if artifact.web_stack.is_none() {
        anyhow::bail!("发布包没有页面凭据 docs/WEB-STACK.v1：不是新格式的 Solid 正式包");
    }
    let receipt = read_receipt(&args.verification_receipt)?;
    package_receipt::check(&artifact, Some(&receipt), &args.package_sha256, now)?;
    Ok(VerifiedPackage {
        artifact,
        tag: receipt.tag,
        archive_sha256: receipt.archive_sha256,
        verified_at: receipt.verified_at,
    })
}

fn read_receipt(path: &Path) -> anyhow::Result<package_receipt::PackageVerification> {
    let bytes = files::read_bounded(path, MAX_RECEIPT_FILE_BYTES)?;
    package_receipt::parse(&bytes)
}
