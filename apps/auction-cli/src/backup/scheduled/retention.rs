//! 按 completed 回执或补充副本完成事实清理旧备份；不重读 archive，不越过副本和文件身份边界。

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use uuid::Uuid;

use super::{
    completion,
    filesystem::{FileIdentity, PrivateDirectory},
    journal::Journal,
    model::{FORMAT, Outcome, RunReceipt},
    offsite::STAGING_DIRECTORY,
};

pub(super) async fn clean(
    journal: &Journal,
    output: &PrivateDirectory,
    current: &mut RunReceipt,
) -> anyhow::Result<()> {
    if !current.archive.as_ref().is_some_and(|archive| archive.verified) {
        anyhow::bail!("本次备份尚未通过校验，禁止本地清理");
    }
    validate_files(output, current)?;
    let mut candidates = BTreeMap::new();
    for name in journal.receipts.names()? {
        tokio::task::yield_now().await;
        let Some(run_id) = receipt_id(&name) else {
            continue;
        };
        let receipt = journal.receipts.read(&name).and_then(|bytes| {
            serde_json::from_slice::<RunReceipt>(&bytes).context("解析已完成回执失败")
        });
        let receipt = match receipt {
            Ok(receipt) if receipt.format == FORMAT && receipt.run_id == run_id => receipt,
            _ => {
                current.cleanup.skip(&name, "receipt_invalid");
                continue;
            }
        };
        if receipt.finished_at.is_none()
            || (receipt.outcome != Outcome::Completed && !receipt.offsite.enabled)
            || receipt.output_dir != output.path
            || receipt.database_name_sha256 != current.database_name_sha256
        {
            continue;
        }
        // 本地校验前失败的运行没有合格备份，不作为副本异常在每次回执中重复告警。
        let Some(archive) = receipt.archive.as_ref().filter(|archive| archive.verified) else {
            continue;
        };
        match completion::allows_cleanup(journal, &receipt) {
            Ok(true) => {}
            Ok(false) => {
                current.cleanup.skip(&name, "replica_not_verified");
                continue;
            }
            Err(_) => {
                current.cleanup.skip(&name, "replica_evidence_invalid");
                continue;
            }
        }
        let expected = receipt.archive_name();
        if archive.archive_file_name != expected
            || archive.manifest_file_name != format!("{expected}.manifest")
        {
            current.cleanup.skip(&name, "archive_name_or_verification_invalid");
            continue;
        }
        candidates.insert(expected, receipt);
    }

    // 只按输出目录的实际目录项认领；符号链接、特殊文件和多硬链接一律不进入候选集。
    let mut eligible = Vec::new();
    let manifests: BTreeSet<_> = candidates
        .values()
        .filter_map(|receipt| {
            receipt.archive.as_ref().map(|archive| archive.manifest_file_name.clone())
        })
        .collect();
    let current_name = current.archive_name();
    let current_manifest = format!("{current_name}.manifest");
    let names: BTreeSet<_> = output.names()?.into_iter().collect();
    for name in &names {
        let name = name.as_str();
        tokio::task::yield_now().await;
        if name == "scheduled.lock"
            || name == STAGING_DIRECTORY
            || name == current_name
            || name == current_manifest
        {
            continue;
        }
        if manifests.contains(name) {
            if let Some(archive_name) = name.strip_suffix(".manifest")
                && !names.contains(archive_name)
            {
                current.cleanup.skip(name, "archive_missing");
            }
            continue;
        }
        let Some(receipt) = candidates.remove(name) else {
            current.cleanup.skip(name, "not_a_completed_scheduled_backup");
            continue;
        };
        if validate_files(output, &receipt).is_err() {
            current.cleanup.skip(name, "file_identity_or_permissions_invalid");
            continue;
        }
        eligible.push(receipt);
    }
    // 历史 completed 回执永久保留；其文件已正常淘汰时不制造虚假的异常跳过项。
    // 当前新备份始终保留，具有完成证据的旧备份按开始时间和 UUID 稳定排序。
    eligible.sort_by_key(|receipt| std::cmp::Reverse((receipt.started_at, receipt.run_id)));
    let keep_old =
        usize::from(current.policy.as_ref().context("缺少备份保留策略")?.local_retention_count) - 1;
    for receipt in eligible.into_iter().skip(keep_old) {
        tokio::task::yield_now().await;
        let Some(archive) = &receipt.archive else {
            continue;
        };
        if validate_files(output, &receipt).is_err() {
            current.cleanup.skip(&archive.archive_file_name, "file_changed_before_removal");
            continue;
        }
        // 先删除 manifest，使半途失败剩下的 archive 不会被误认为完整恢复点。
        if output.remove(&archive.manifest_file_name, archive.manifest_identity).is_err()
            || output.remove(&archive.archive_file_name, archive.archive_identity).is_err()
        {
            current.cleanup.skip(&archive.archive_file_name, "remove_failed");
            anyhow::bail!("本地保留清理失败；剩余文件已保留");
        }
        current.cleanup.removed(archive.archive_file_name.clone());
    }
    Ok(())
}

fn receipt_id(name: &str) -> Option<Uuid> {
    let run_id = Uuid::parse_str(name.strip_suffix(".json")?).ok()?;
    (name == format!("{run_id}.json") && !run_id.is_nil()).then_some(run_id)
}

fn validate_files(output: &PrivateDirectory, receipt: &RunReceipt) -> anyhow::Result<()> {
    let archive = receipt.archive.as_ref().context("回执缺少 archive")?;
    for (name, expected) in [
        (&archive.archive_file_name, archive.archive_identity),
        (&archive.manifest_file_name, archive.manifest_identity),
    ] {
        if FileIdentity::of(&output.open(name)?.metadata()?) != expected {
            anyhow::bail!("备份文件已被替换或改变大小");
        }
    }
    Ok(())
}
