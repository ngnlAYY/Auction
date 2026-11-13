//! 恢复后外部核对：暂停 Worker 下只读扫描 Sent / Read Mail / 论坛新楼层，
//! 把恢复库中仍为 pending/claimed 且与 Sent 匹配的投递隔离为人工对账，并把无法唯一归属
//! Sent 的投递显式阻塞。
//!
//! 本命令只读取远端列表、为候选 MID 读取详情，并在显式 `--apply` 时写入隔离事实或本地阻塞；
//! 绝不重发、领取、标记已读或改写资金与结算事实。恢复库没有当前 fence 冻结发送尝试时，
//! 身份只能用 payload 重建（世界/收件人/资产，没有渲染标题与正文摘要），结论按“未决”处理。
//! Read Mail 没有持久读取时间，因此不做到达时间截断，缺失项一律按恢复库身份报告。
//! Worker 未暂停、能力不可用或证据不完整时明确失败或把回执标记为不完整，不能用“未观察到”
//! 推断“未发送”。

use std::{collections::BTreeSet, path::Path, time::Duration};

use anyhow::Context;
use auction_application::operations::{
    WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS, WorkerRecoveryScanRequest, WorkerRecoveryScanResponse,
    WorkerRemoteReadError, WorkerRemoteReadPort,
};
use auction_application::recovery::{
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, ExternalRecoveryStore, MAX_EXTERNAL_RECOVERY_DELIVERIES,
};
use auction_config::AppConfig;
use auction_storage::DatabaseStore;
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

mod sent_plan;

use sent_plan::{SentMatchPlan, apply_scan_blockers, evidence_label, missing_inbound};

use crate::app::print_json;
use crate::cli::RecoveryCommand;
use crate::config_dir::ConfigDir;
use crate::support::files;

/// `--apply` 的逐字确认短语。
const APPLY_CONFIRMATION: &str = "APPLY EXTERNAL RECONCILIATION";
/// 允许的恢复点时钟偏差；超过即拒绝，避免把未来时刻当作恢复点。
const CUTOFF_FUTURE_TOLERANCE_SECONDS: i64 = 300;

pub(crate) async fn run(config_dir: &ConfigDir, command: RecoveryCommand) -> anyhow::Result<()> {
    match command {
        RecoveryCommand::ExternalReconcile {
            cutoff,
            receipt,
            maximum_pages,
            maximum_forum_threads,
            apply,
            confirm,
            actor,
            scan_id,
        } => {
            external_reconcile(
                config_dir,
                &cutoff,
                &receipt,
                maximum_pages,
                maximum_forum_threads,
                apply,
                confirm.as_deref(),
                actor.as_deref(),
                scan_id,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn external_reconcile(
    config_dir: &ConfigDir,
    cutoff: &str,
    receipt_path: &Path,
    maximum_pages: u32,
    maximum_forum_threads: u16,
    apply: bool,
    confirm: Option<&str>,
    actor: Option<&str>,
    scan_id: Option<Uuid>,
) -> anyhow::Result<()> {
    let cutoff_at = DateTime::parse_from_rfc3339(cutoff)
        .context("恢复点必须是 RFC 3339 时间（含时区）")?
        .with_timezone(&Utc);
    if cutoff_at > Utc::now() + TimeDelta::seconds(CUTOFF_FUTURE_TOLERANCE_SECONDS) {
        anyhow::bail!("恢复点不能晚于当前时刻");
    }
    let actor = resolve_apply_authorization(apply, confirm, actor)?;
    let scan_id = scan_id.unwrap_or_else(Uuid::new_v4);
    files::require_private_target(receipt_path)?;

    let (config, store) =
        super::worker_store(config_dir, auction_storage::DatabaseConnectionPurpose::CliOperations)
            .await?;
    let result = execute(
        &config,
        store.as_ref(),
        cutoff_at,
        receipt_path,
        maximum_pages,
        maximum_forum_threads,
        apply,
        actor.as_deref(),
        scan_id,
    )
    .await;
    store.postgres_pool().close().await;
    result
}

/// `--apply` 必须同时提供逐字确认与操作者；其余情况不得写数据库。
fn resolve_apply_authorization(
    apply: bool,
    confirm: Option<&str>,
    actor: Option<&str>,
) -> anyhow::Result<Option<String>> {
    if !apply {
        if confirm.is_some() || actor.is_some() {
            anyhow::bail!("未指定 --apply 时不能提供 --confirm 或 --actor");
        }
        return Ok(None);
    }
    if confirm != Some(APPLY_CONFIRMATION) {
        anyhow::bail!("--apply 需要逐字确认 --confirm '{APPLY_CONFIRMATION}'");
    }
    let actor = actor.context("--apply 需要 --actor 操作者标识")?;
    if actor.is_empty()
        || actor.len() > 128
        || actor != actor.trim()
        || actor.chars().any(char::is_control)
    {
        anyhow::bail!("--actor 必须是 1-128 字节、无控制字符的文本");
    }
    Ok(Some(actor.to_owned()))
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    config: &AppConfig,
    store: &DatabaseStore,
    cutoff_at: DateTime<Utc>,
    receipt_path: &Path,
    maximum_pages: u32,
    maximum_forum_threads: u16,
    apply: bool,
    actor: Option<&str>,
    scan_id: Uuid,
) -> anyhow::Result<()> {
    let expectations =
        store.pending_claimed_send_expectations().await.context("读取待核对投递发送预期失败")?;
    if expectations.iter().any(|expectation| !expectation.validate()) {
        anyhow::bail!("恢复库中的投递发送预期违反发送不变量");
    }
    let forum_threads = store
        .published_forum_threads(maximum_forum_threads)
        .await
        .context("读取已发布论坛主题失败")?;

    let worlds = configured_mail_worlds(config);
    let client = auction_integrations::WorkerRemoteReadClient::new(
        config.worker_control_socket_path(),
        Duration::from_secs(WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS),
    )
    .map_err(|error| anyhow::anyhow!("Worker control client 不可用：{error}"))?;

    let first = client
        .recovery_scan(WorkerRecoveryScanRequest {
            cutoff_at,
            maximum_pages,
            sent_worlds: worlds.clone(),
            read_mail_worlds: worlds.clone(),
            // 即使没有配置世界也发送扫描标志：空世界列表是合法的“无远端目标”，
            // 不能让请求因所有 include_* 都为 false 被 Worker 拒绝。
            include_sent: true,
            include_read_mail: true,
            include_forum: !forum_threads.is_empty(),
            sent_details: Vec::new(),
            forum_threads: forum_threads
                .iter()
                .map(|thread| auction_application::operations::WorkerRecoveryForumTarget {
                    world: thread.world_id.clone(),
                    thread_id: thread.thread_id.clone(),
                    after_post_number: thread.after_post_number,
                })
                .collect(),
        })
        .await
        .map_err(remote_scan_error)?;

    let mut plan = SentMatchPlan::new(&expectations, cutoff_at);
    plan.scan_entries(&first.sent);
    let mut second: Option<WorkerRecoveryScanResponse> = None;
    if !plan.detail_refs.is_empty() {
        let response = client
            .recovery_scan(WorkerRecoveryScanRequest {
                cutoff_at,
                maximum_pages,
                sent_worlds: Vec::new(),
                read_mail_worlds: Vec::new(),
                include_sent: false,
                include_read_mail: false,
                include_forum: false,
                sent_details: plan.detail_refs.iter().cloned().collect(),
                forum_threads: Vec::new(),
            })
            .await
            .map_err(remote_scan_error)?;
        plan.apply_details(&response.sent_details);
        second = Some(response);
    }

    let read_mail_candidates = first
        .read_mail
        .iter()
        .map(|entry| (entry.world.clone(), entry.external_mail_id.clone()))
        .collect::<Vec<_>>();
    let known = store
        .known_inbound_mail_ids(read_mail_candidates)
        .await
        .context("读取恢复库已知入站邮件失败")?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let missing_inbound = missing_inbound(&first.read_mail, &known);

    // 暂停是隔离证据的前置条件：任一轮在调用期间恢复外部访问，结果都不能用于写入。
    let scan_paused = first.paused && second.as_ref().is_none_or(|response| response.paused);
    if apply {
        let detail_errors =
            second.as_ref().map(|response| response.errors.as_slice()).unwrap_or(&[]);
        let blockers = apply_scan_blockers(
            scan_paused,
            first.sent_complete,
            &first.errors,
            detail_errors,
            !plan.detail_refs.is_empty() && second.is_none(),
            expectations.len() >= MAX_EXTERNAL_RECOVERY_DELIVERIES,
        );
        if !blockers.is_empty() {
            anyhow::bail!(
                "恢复核对不完整，拒绝 --apply；请先补齐扫描并重新核对：{}",
                blockers.join(",")
            );
        }
    }
    let quarantine_matches = plan.quarantine_matches();
    let unresolved_blocks = plan.unresolved_blocks();
    let mut outcomes = Vec::new();
    let mut blocked = Vec::new();
    if apply {
        let actor = actor.context("--apply 缺少操作者")?;
        if !quarantine_matches.is_empty() {
            let applied = store
                .quarantine_external_sent_matches(scan_id, actor.to_owned(), quarantine_matches)
                .await
                .context("隔离精确匹配投递失败")?;
            outcomes = applied.iter().map(outcome_projection).collect();
        }
        // 无法唯一归属 Sent 的投递必须显式阻塞：备份后可能已经寄出，继续 pending 会被重发。
        if !unresolved_blocks.is_empty() {
            let applied = store
                .block_unresolved_send_deliveries(scan_id, actor.to_owned(), unresolved_blocks)
                .await
                .context("阻塞未决投递失败")?;
            blocked = applied.iter().map(outcome_projection).collect();
        }
    }

    let receipt = json!({
        "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
        "generated_at": Utc::now(),
        "scan_id": scan_id,
        "cutoff_at": cutoff_at,
        "dry_run": !apply,
        "worker": {
            "paused": scan_paused,
            "status": first.status,
            "observed_at": first.observed_at,
            "sent_complete": first.sent_complete,
            "read_mail_complete": first.read_mail_complete,
            "pages_scanned": first.pages_scanned,
            "errors": first.errors,
            "detail_errors": second.as_ref().map_or_else(Vec::new, |response| response.errors.clone()),
        },
        "local": {
            "pending_claimed_deliveries": expectations.len(),
            "pending_claimed_truncated":
                expectations.len() >= MAX_EXTERNAL_RECOVERY_DELIVERIES,
            "frozen_expectations": plan.frozen_count,
            "restored_payload_expectations": plan.restored_payload_count,
            "published_forum_threads": forum_threads.len(),
        },
        "sent": {
            "entries": first.sent.len(),
            "details_read": second.as_ref().map_or(0, |response| response.sent_details.len()),
            "candidates": plan.candidates,
            "exact": plan.exact.len(),
            "ambiguous": plan.ambiguous,
            "asset_mismatch": plan.asset_mismatch,
            "identity_mismatch": plan.identity_mismatch,
            "unconfirmed": plan.unconfirmed,
        },
        "deliveries": {
            "planned": plan.exact.iter().map(|exact| json!({
                "delivery_id": exact.delivery_id,
                "fencing_version": exact.fencing_version,
                "evidence": evidence_label(exact.evidence),
                "world": exact.world_id,
                "external_mail_id": exact.external_mail_id,
                "sent_at": exact.sent_at,
            })).collect::<Vec<_>>(),
            "quarantined": outcomes,
            "unresolved": plan.unresolved_projections(),
            "blocked": blocked,
            "payload_evidence_note": "restored_payload 身份来自恢复库 payload，没有可证明的标题/正文摘要，只用世界、收件人与资产核对",
        },
        "read_mail": {
            "scanned": first.read_mail.len(),
            // 读取时间没有持久证据：不用到达时间截断，否则备份前到达、备份后才读取的邮件会漏报。
            "arrival_cutoff_applied": false,
            "missing_locally": missing_inbound,
        },
        "forum": {
            "threads_scanned": forum_threads.len(),
            "missing_posts": first.forum.iter().map(|post| json!({
                "world": post.world,
                "thread_id": post.thread_id,
                "post_id": post.post_id,
                "post_number": post.post_number,
            })).collect::<Vec<_>>(),
        },
    });
    write_receipt(receipt_path, &receipt)?;
    print_json(&json!({
        "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
        "scan_id": scan_id,
        "receipt": receipt_path,
        "dry_run": !apply,
        "planned_quarantines": plan.exact.len(),
        "planned_unresolved_blocks": plan.unresolved_blocks().len(),
        "quarantined": outcomes.len(),
        "blocked": blocked.len(),
        "missing_inbound": receipt["read_mail"]["missing_locally"].as_array().map_or(0, Vec::len),
        "missing_forum_posts": first.forum.len(),
    }))
}

fn outcome_projection(
    outcome: &auction_application::recovery::ExternalRecoveryQuarantineOutcome,
) -> Value {
    json!({
        "delivery_id": outcome.delivery_id,
        "status": outcome.status,
        "fencing_version": outcome.fencing_version,
        "reason": outcome.reason,
    })
}

fn write_receipt(path: &Path, receipt: &Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(receipt).context("序列化恢复核对回执失败")?;
    files::write_private(path, &bytes)
}

fn remote_scan_error(error: WorkerRemoteReadError) -> anyhow::Error {
    match error {
        WorkerRemoteReadError::NotPaused => anyhow::anyhow!(
            "外部访问未暂停：先用 `auction-cli switch set external_access_paused true` 暂停 Worker"
        ),
        other => anyhow::anyhow!("Worker 恢复扫描不可用：{}", other.as_str()),
    }
}

/// 配置中实际启用的 MoogleMail 世界；顺序稳定且去重。
fn configured_mail_worlds(config: &AppConfig) -> Vec<String> {
    if !config.worker.mooglemail.enabled {
        return Vec::new();
    }
    let mut worlds = config
        .worker
        .mooglemail
        .configured_worlds()
        .into_iter()
        .map(|world| world.world_id)
        .filter(|world| matches!(world.as_str(), "main" | "isekai"))
        .collect::<Vec<_>>();
    worlds.sort();
    worlds.dedup();
    worlds
}

#[cfg(test)]
mod tests;
