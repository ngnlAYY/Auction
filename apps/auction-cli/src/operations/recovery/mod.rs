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
//!
//! 不变量：隔离与阻塞是两个独立事务。隔离已提交而阻塞失败时，先写回执（`apply_outcome =
//! block_failed`）再返回错误；两步的错误都带 scan_id，同一 `--scan-id` 原样重放时已处理项
//! 幂等跳过。

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
use auction_time::{TimeDelta, Timestamp};
use serde_json::{Value, json};
use uuid::Uuid;

mod sent_plan;

use crate::app::print_json;
use crate::cli::RecoveryCommand;
use crate::config_dir::ConfigDir;
use crate::support::{files, text};
use sent_plan::{SentMatchPlan, apply_scan_blockers, evidence_label, missing_inbound};

/// `--apply` 的逐字确认短语。
const APPLY_CONFIRMATION: &str = "APPLY EXTERNAL RECONCILIATION";
/// 允许的恢复点时钟偏差；超过即拒绝，避免把未来时刻当作恢复点。
const CUTOFF_FUTURE_TOLERANCE_SECONDS: i64 = 300;

/// 一次外部核对的已校验参数；`actor` 为 Some 表示 `--apply` 已通过确认短语与操作者校验。
struct ReconcileOptions<'a> {
    cutoff_at: Timestamp,
    receipt_path: &'a Path,
    maximum_pages: u32,
    maximum_forum_threads: u16,
    actor: Option<&'a str>,
    scan_id: Uuid,
}

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
            let cutoff_at = Timestamp::parse_from_rfc3339(&cutoff)
                .context("恢复点必须是 RFC 3339 时间（含时区）")?;
            if cutoff_at > Timestamp::now() + TimeDelta::seconds(CUTOFF_FUTURE_TOLERANCE_SECONDS) {
                anyhow::bail!("恢复点不能晚于当前时刻");
            }
            let actor = resolve_apply_authorization(apply, confirm.as_deref(), actor.as_deref())?;
            files::require_private_target(&receipt)?;
            let options = ReconcileOptions {
                cutoff_at,
                receipt_path: &receipt,
                maximum_pages,
                maximum_forum_threads,
                actor: actor.as_deref(),
                scan_id: scan_id.unwrap_or_else(Uuid::new_v4),
            };
            super::with_worker_store(
                config_dir,
                auction_storage::DatabaseConnectionPurpose::CliOperations,
                async |config, store| execute(config, store, &options).await,
            )
            .await
        }
    }
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
    if !text::single_line(actor, 128) {
        anyhow::bail!("--actor 必须是 1-128 字节、无控制字符的文本");
    }
    Ok(Some(actor.to_owned()))
}

/// 两阶段远端扫描与本地对照的事实；回执与 `--apply` 共用同一份。
struct ScanFacts {
    expectations: usize,
    forum_threads: usize,
    first: WorkerRecoveryScanResponse,
    second: Option<WorkerRecoveryScanResponse>,
    plan: SentMatchPlan,
    missing_inbound: Vec<Value>,
    /// 暂停是隔离证据的前置条件：任一轮在调用期间恢复外部访问，结果都不能用于写入。
    paused: bool,
}

/// `--apply` 的写入结果。演练时两个列表为空、`outcome` 为 None。
#[derive(Default)]
struct Applied {
    quarantined: Vec<Value>,
    blocked: Vec<Value>,
    outcome: Option<&'static str>,
}

async fn execute(
    config: &AppConfig,
    store: &DatabaseStore,
    options: &ReconcileOptions<'_>,
) -> anyhow::Result<()> {
    let facts = scan(config, store, options).await?;
    let (applied, apply_error) = match options.actor {
        Some(actor) => {
            require_complete_scan(&facts)?;
            apply(store, &facts.plan, options.scan_id, actor).await?
        }
        None => (Applied::default(), None),
    };
    let receipt = build_receipt(&facts, &applied, options);
    let written = write_receipt(options.receipt_path, &receipt)
        .with_context(|| format!("恢复核对回执写入失败（scan_id={}）", options.scan_id));
    if let Some(error) = apply_error {
        // 隔离已提交、阻塞失败：回执已如实标记阻塞未完成，随后仍返回阻塞的原错误。
        return Err(match written {
            Ok(()) => error.context(format!(
                "隔离已提交、阻塞未完成；回执 {} 标记 apply_outcome=block_failed",
                options.receipt_path.display()
            )),
            Err(write_error) => anyhow::anyhow!("{error:#}；{write_error:#}"),
        });
    }
    written?;
    print_json(&json!({
        "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
        "scan_id": options.scan_id,
        "receipt": options.receipt_path,
        "dry_run": options.actor.is_none(),
        "planned_quarantines": facts.plan.exact.len(),
        "planned_unresolved_blocks": facts.plan.unresolved_blocks().len(),
        "quarantined": applied.quarantined.len(),
        "blocked": applied.blocked.len(),
        "missing_inbound": facts.missing_inbound.len(),
        "missing_forum_posts": facts.first.forum.len(),
    }))
}

async fn scan(
    config: &AppConfig,
    store: &DatabaseStore,
    options: &ReconcileOptions<'_>,
) -> anyhow::Result<ScanFacts> {
    let expectations =
        store.pending_claimed_send_expectations().await.context("读取待核对投递发送预期失败")?;
    if expectations.iter().any(|expectation| !expectation.validate()) {
        anyhow::bail!("恢复库中的投递发送预期违反发送不变量");
    }
    let forum_threads = store
        .published_forum_threads(options.maximum_forum_threads)
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
            cutoff_at: options.cutoff_at,
            maximum_pages: options.maximum_pages,
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

    let mut plan = SentMatchPlan::new(&expectations, options.cutoff_at);
    plan.scan_entries(&first.sent);
    let mut second: Option<WorkerRecoveryScanResponse> = None;
    if !plan.detail_refs.is_empty() {
        let response = client
            .recovery_scan(WorkerRecoveryScanRequest {
                cutoff_at: options.cutoff_at,
                maximum_pages: options.maximum_pages,
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
    let paused = first.paused && second.as_ref().is_none_or(|response| response.paused);
    Ok(ScanFacts {
        expectations: expectations.len(),
        forum_threads: forum_threads.len(),
        first,
        second,
        plan,
        missing_inbound,
        paused,
    })
}

/// `--apply` 的写入前提：扫描期间保持暂停、Sent 列表完整、详情全部读到且候选未被截断。
fn require_complete_scan(facts: &ScanFacts) -> anyhow::Result<()> {
    let detail_errors =
        facts.second.as_ref().map_or(&[][..], |response| response.errors.as_slice());
    let blockers = apply_scan_blockers(
        facts.paused,
        facts.first.sent_complete,
        &facts.first.errors,
        detail_errors,
        !facts.plan.detail_refs.is_empty() && facts.second.is_none(),
        facts.expectations >= MAX_EXTERNAL_RECOVERY_DELIVERIES,
    );
    if !blockers.is_empty() {
        anyhow::bail!(
            "恢复核对不完整，拒绝 --apply；请先补齐扫描并重新核对：{}",
            blockers.join(",")
        );
    }
    Ok(())
}

/// 依次提交隔离与阻塞两个事务。隔离失败或没有隔离可提交时直接返回错误；隔离已提交而阻塞
/// 失败时返回 `block_failed` 结果与阻塞错误，由调用方先写回执再返回错误。
async fn apply(
    store: &DatabaseStore,
    plan: &SentMatchPlan,
    scan_id: Uuid,
    actor: &str,
) -> anyhow::Result<(Applied, Option<anyhow::Error>)> {
    let quarantine_matches = plan.quarantine_matches();
    let unresolved_blocks = plan.unresolved_blocks();
    let mut applied = Applied { outcome: Some("completed"), ..Applied::default() };
    let quarantine_committed = !quarantine_matches.is_empty();
    if quarantine_committed {
        let outcomes = store
            .quarantine_external_sent_matches(scan_id, actor.to_owned(), quarantine_matches)
            .await
            .with_context(|| replay_hint("隔离精确匹配投递失败", scan_id))?;
        applied.quarantined = outcomes.iter().map(outcome_projection).collect();
    }
    // 无法唯一归属 Sent 的投递必须显式阻塞：备份后可能已经寄出，继续 pending 会被重发。
    if !unresolved_blocks.is_empty() {
        match store
            .block_unresolved_send_deliveries(scan_id, actor.to_owned(), unresolved_blocks)
            .await
        {
            Ok(outcomes) => applied.blocked = outcomes.iter().map(outcome_projection).collect(),
            Err(error) => {
                let error =
                    anyhow::Error::new(error).context(replay_hint("阻塞未决投递失败", scan_id));
                if !quarantine_committed {
                    return Err(error);
                }
                applied.outcome = Some("block_failed");
                return Ok((applied, Some(error)));
            }
        }
    }
    Ok((applied, None))
}

/// 写入步骤的错误都带 scan_id：结果可能未知，只能用同一 scan_id 原样重放。
fn replay_hint(what: &str, scan_id: Uuid) -> String {
    format!(
        "{what}（scan_id={scan_id}）；结果可能未知，用 --scan-id {scan_id} 与新的 --receipt \
         路径原样重放，已处理项幂等跳过"
    )
}

fn build_receipt(facts: &ScanFacts, applied: &Applied, options: &ReconcileOptions<'_>) -> Value {
    let first = &facts.first;
    let second = facts.second.as_ref();
    let plan = &facts.plan;
    json!({
        "format": EXTERNAL_RECOVERY_RECEIPT_FORMAT,
        "generated_at": Timestamp::now(),
        "scan_id": options.scan_id,
        "cutoff_at": options.cutoff_at,
        "dry_run": options.actor.is_none(),
        // 演练为 null；completed 表示隔离与阻塞都已提交，block_failed 表示隔离已提交、阻塞
        // 返回错误（结果可能未知），须用同一 scan_id 重放。
        "apply_outcome": applied.outcome,
        "worker": {
            "paused": facts.paused,
            "status": first.status,
            "observed_at": first.observed_at,
            "sent_complete": first.sent_complete,
            "read_mail_complete": first.read_mail_complete,
            "pages_scanned": first.pages_scanned,
            "errors": first.errors,
            "detail_errors": second.map_or_else(Vec::new, |response| response.errors.clone()),
        },
        "local": {
            "pending_claimed_deliveries": facts.expectations,
            "pending_claimed_truncated": facts.expectations >= MAX_EXTERNAL_RECOVERY_DELIVERIES,
            "frozen_expectations": plan.frozen_count,
            "restored_payload_expectations": plan.restored_payload_count,
            "published_forum_threads": facts.forum_threads,
        },
        "sent": {
            "entries": first.sent.len(),
            "details_read": second.map_or(0, |response| response.sent_details.len()),
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
            "quarantined": applied.quarantined,
            "unresolved": plan.unresolved_projections(),
            "blocked": applied.blocked,
            "payload_evidence_note": "restored_payload 身份来自恢复库 payload，没有可证明的标题/正文摘要，只用世界、收件人与资产核对",
        },
        "read_mail": {
            "scanned": first.read_mail.len(),
            // 读取时间没有持久证据：不用到达时间截断，否则备份前到达、备份后才读取的邮件会漏报。
            "arrival_cutoff_applied": false,
            "missing_locally": facts.missing_inbound,
        },
        "forum": {
            "threads_scanned": facts.forum_threads,
            "missing_posts": first.forum.iter().map(|post| json!({
                "world": post.world,
                "thread_id": post.thread_id,
                "post_id": post.post_id,
                "post_number": post.post_number,
            })).collect::<Vec<_>>(),
        },
    })
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
            "外部访问未暂停：先按 ops/deploy/RECOVERY.md「外部历史核对与启动」第 1 步执行 \
             `auction-cli switch set external_access_paused true --expires-at <RFC3339> \
             --command-id <稳定UUID> --expected-revision <switch list 中的版本> \
             --actor <操作者说明> --reason <非敏感原因>` 暂停 Worker"
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
