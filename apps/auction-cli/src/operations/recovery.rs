//! W10.9 恢复后外部核对：暂停 Worker 下只读扫描 Sent / Read Mail / 论坛新楼层，
//! 把恢复库中仍为 pending/claimed 且与 Sent 匹配的投递隔离为人工对账，并把无法唯一归属
//! Sent 的投递显式阻塞。
//!
//! 本命令只读取远端列表、为候选 MID 读取详情，并在显式 `--apply` 时写入隔离事实或本地阻塞；
//! 绝不重发、领取、标记已读或改写资金与结算事实。恢复库没有当前 fence 冻结发送尝试时，
//! 身份只能用 payload 重建（世界/收件人/资产，没有渲染标题与正文摘要），结论按“未决”处理。
//! Read Mail 没有持久读取时间，因此不做到达时间截断，缺失项一律按恢复库身份报告。
//! Worker 未暂停、能力不可用或证据不完整时明确失败或把回执标记为不完整，不能用“未观察到”
//! 推断“未发送”。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use anyhow::Context;
use auction_application::operations::{
    WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS, WorkerRecoveryInboundEntry, WorkerRecoveryMailRef,
    WorkerRecoveryScanRequest, WorkerRecoveryScanResponse, WorkerRemoteReadError,
    WorkerRemoteReadPort, WorkerSentLookupCandidate,
};
use auction_application::recovery::{
    EXTERNAL_RECOVERY_AMBIGUOUS_REASON, EXTERNAL_RECOVERY_RECEIPT_FORMAT,
    ExternalRecoveryDeliveryExpectation, ExternalRecoveryQuarantineMatch,
    ExternalRecoverySendEvidence, ExternalRecoveryStore, ExternalRecoveryUnresolvedBlock,
    MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS, MAX_EXTERNAL_RECOVERY_DELIVERIES, SentDetailMatch,
    sent_detail_match, sent_list_projection_may_match,
};
use auction_config::AppConfig;
use auction_storage::DatabaseStore;
use chrono::{DateTime, TimeDelta, Timelike, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app::print_json;
use crate::cli::RecoveryCommand;
use crate::deployment::ConfigDir;
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

    if apply {
        let detail_errors =
            second.as_ref().map(|response| response.errors.as_slice()).unwrap_or(&[]);
        let blockers = apply_scan_blockers(
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
            "paused": first.paused && second.as_ref().is_none_or(|response| response.paused),
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

/// 精确匹配计划；只在 `--apply` 时转换为隔离匹配。
#[derive(Debug, Default)]
struct SentMatchPlan {
    expectations: Vec<ExternalRecoveryDeliveryExpectation>,
    /// 原始恢复截止；精确匹配只接受不早于该时刻的详情。
    cutoff_at: Option<DateTime<Utc>>,
    /// Sent 分钟精度导致的候选下界；只用于筛选候选，不能用于宣称精确匹配。
    scan_from: Option<DateTime<Utc>>,
    /// 有列表候选但尚未读取详情的投递 → 候选 MID。
    pending_details: Vec<(usize, SentListCandidate)>,
    detail_refs: BTreeSet<WorkerRecoveryMailRef>,
    exact: Vec<ExactDeliveryMatch>,
    /// 每条投递观察到的 Sent 列表候选 MID；只用于回执与未决阻塞细节。
    observed: BTreeMap<usize, BTreeSet<String>>,
    /// 无法唯一归属 Sent 的投递；`--apply` 必须把它们显式阻塞，而不是继续可领取。
    ambiguous_deliveries: BTreeSet<usize>,
    /// 有列表候选但详情缺失/不唯一而无法确认的投递；同样不能留在 pending。
    unconfirmed_deliveries: BTreeSet<usize>,
    /// 有冻结发送尝试的投递数；其余只能用恢复库 payload 重建身份。
    frozen_count: usize,
    restored_payload_count: usize,
    candidates: usize,
    ambiguous: usize,
    asset_mismatch: usize,
    identity_mismatch: usize,
    /// 有列表候选但第二阶段没有返回详情（能力失败或截断）。
    unconfirmed: usize,
}

#[derive(Debug, Clone)]
struct SentListCandidate {
    world_id: String,
    external_mail_id: String,
}

#[derive(Debug, Clone)]
struct ExactDeliveryMatch {
    delivery_id: Uuid,
    fencing_version: i64,
    evidence: ExternalRecoverySendEvidence,
    world_id: String,
    external_mail_id: String,
    sent_at: DateTime<Utc>,
    recipient: String,
    subject: String,
    expected_credits: i64,
    expected_cod: i64,
    expected_equipment_ids: Option<Vec<String>>,
    collected: bool,
    observed_at: DateTime<Utc>,
}

/// `--apply` 前必须证明 Sent 列表与详情完整，且待核对投递未被截断。
fn apply_scan_blockers(
    sent_complete: bool,
    scan_errors: &[String],
    detail_errors: &[String],
    detail_unavailable: bool,
    truncated: bool,
) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if !sent_complete {
        blockers.push("sent_scan_incomplete");
    }
    if !scan_errors.is_empty() {
        blockers.push("sent_scan_errors");
    }
    if !detail_errors.is_empty() {
        blockers.push("sent_detail_errors");
    }
    if detail_unavailable {
        blockers.push("sent_detail_unavailable");
    }
    if truncated {
        blockers.push("pending_claimed_truncated");
    }
    blockers
}

/// Sent 页面只有分钟精度；恢复截止按所在分钟下界比较，保留整个重叠分钟。
fn cutoff_minute(cutoff_at: DateTime<Utc>) -> DateTime<Utc> {
    cutoff_at
        - TimeDelta::seconds(i64::from(cutoff_at.second()))
        - TimeDelta::nanoseconds(i64::from(cutoff_at.timestamp_subsec_nanos()))
}

impl SentMatchPlan {
    fn new(expectations: &[ExternalRecoveryDeliveryExpectation], cutoff_at: DateTime<Utc>) -> Self {
        let frozen_count = expectations
            .iter()
            .filter(|expectation| expectation.evidence == ExternalRecoverySendEvidence::Frozen)
            .count();
        Self {
            expectations: expectations.to_vec(),
            // Sent 列表/详情只精确到分钟；分钟下界只扩大候选，精确匹配仍使用原始 cutoff。
            // 与 cutoff 重叠的分钟无法证明实际发送时刻，后续必须进入未决阻塞。
            cutoff_at: Some(cutoff_at),
            scan_from: Some(cutoff_minute(cutoff_at)),
            frozen_count,
            restored_payload_count: expectations.len().saturating_sub(frozen_count),
            ..Self::default()
        }
    }

    /// 先用列表投影筛选详情候选；只有唯一候选才进入第二阶段，多候选即未决。
    fn scan_entries(
        &mut self,
        entries: &[auction_application::operations::WorkerRecoverySentEntry],
    ) {
        let Some(cutoff_at) = self.scan_from else { return };
        for (index, expectation) in self.expectations.iter().enumerate() {
            let candidates = entries
                .iter()
                .filter(|entry| {
                    sent_list_projection_may_match(
                        expectation,
                        &entry.world,
                        &entry.recipient,
                        &entry.subject,
                        entry.sent_at,
                        cutoff_at,
                    )
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let mut observed = BTreeSet::new();
            for entry in &candidates {
                observed.insert(entry.external_mail_id.clone());
            }
            self.observed.insert(index, observed);
            match candidates.as_slice() {
                [entry] => {
                    self.candidates += 1;
                    self.pending_details.push((
                        index,
                        SentListCandidate {
                            world_id: entry.world.clone(),
                            external_mail_id: entry.external_mail_id.clone(),
                        },
                    ));
                    self.detail_refs.insert(WorkerRecoveryMailRef {
                        world: entry.world.clone(),
                        external_mail_id: entry.external_mail_id.clone(),
                    });
                }
                _ => {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(index);
                }
            }
        }
    }

    fn apply_details(&mut self, details: &[WorkerSentLookupCandidate]) {
        let Some(cutoff_at) = self.cutoff_at else { return };
        let mut by_mid: BTreeMap<&str, Vec<&WorkerSentLookupCandidate>> = BTreeMap::new();
        for detail in details {
            by_mid.entry(detail.external_mail_id.as_str()).or_default().push(detail);
        }
        let pending = std::mem::take(&mut self.pending_details);
        let mut matches = Vec::new();
        for (delivery_index, candidate) in pending {
            let Some(details) = by_mid.get(candidate.external_mail_id.as_str()) else {
                self.unconfirmed += 1;
                self.unconfirmed_deliveries.insert(delivery_index);
                continue;
            };
            if details.len() != 1 {
                self.ambiguous += 1;
                self.ambiguous_deliveries.insert(delivery_index);
                continue;
            }
            let detail = details[0];
            let Some(sent_at) = detail.sent_at else {
                self.unconfirmed += 1;
                self.unconfirmed_deliveries.insert(delivery_index);
                continue;
            };
            // 详情与列表一样只到分钟；早于原始 cutoff 但在同一分钟的 Sent 无法证明
            // 实际发送时刻，不能当作精确匹配，必须显式阻塞。
            let scan_from = self.scan_from.unwrap_or(cutoff_at);
            if sent_at < cutoff_at {
                if sent_at >= scan_from {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(delivery_index);
                } else {
                    self.identity_mismatch += 1;
                }
                continue;
            }
            let expectation = &self.expectations[delivery_index];
            match sent_detail_match(
                expectation,
                &candidate.world_id,
                &detail.recipient,
                &detail.subject,
                sent_at,
                cutoff_at,
                detail.collected,
                detail.remaining_credits,
                detail.cod_pending,
                &detail.remaining_equipment_ids,
            ) {
                SentDetailMatch::Exact => {
                    // 未领取 pending 投递的 fence=0；隔离证据表要求 fence>=1，且不能把
                    // 隔离前 fence 静默改写成 1。此类精确匹配改走未决阻塞（manual_reconcile），
                    // 审计保留原 fence=0，绝不重发。
                    if self.expectations[delivery_index].fencing_version == 0 {
                        self.ambiguous_deliveries.insert(delivery_index);
                    } else {
                        matches.push((delivery_index, candidate, detail.clone(), sent_at));
                    }
                }
                SentDetailMatch::AssetMismatch => self.asset_mismatch += 1,
                SentDetailMatch::IdentityMismatch => self.identity_mismatch += 1,
                // 候选不唯一，或弱证据（payload 重建）无法归因到已领取详情：不能留在
                // pending，必须进入未决阻塞。
                SentDetailMatch::Ambiguous => {
                    self.ambiguous += 1;
                    self.ambiguous_deliveries.insert(delivery_index);
                }
            }
        }
        // 同一 Sent MID 只能证明一个投递；共用证据时两者都不隔离，而是显式阻塞。
        let mut mid_counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_, _, detail, _) in &matches {
            *mid_counts.entry(detail.external_mail_id.clone()).or_default() += 1;
        }
        for (delivery_index, candidate, detail, sent_at) in matches {
            if mid_counts.get(&detail.external_mail_id).copied().unwrap_or(0) > 1 {
                self.ambiguous += 1;
                self.ambiguous_deliveries.insert(delivery_index);
                continue;
            }
            let expectation = &self.expectations[delivery_index];
            self.exact.push(ExactDeliveryMatch {
                delivery_id: expectation.delivery_id,
                fencing_version: expectation.fencing_version,
                evidence: expectation.evidence,
                world_id: candidate.world_id,
                external_mail_id: detail.external_mail_id,
                sent_at,
                recipient: detail.recipient,
                subject: detail.subject,
                expected_credits: expectation.credits,
                expected_cod: expectation.cod,
                expected_equipment_ids: expectation.equipment_ids.clone(),
                collected: detail.collected,
                observed_at: detail.observed_at,
            });
        }
    }

    fn quarantine_matches(&self) -> Vec<ExternalRecoveryQuarantineMatch> {
        self.exact
            .iter()
            .map(|exact| ExternalRecoveryQuarantineMatch {
                delivery_id: exact.delivery_id,
                fencing_version: exact.fencing_version,
                world_id: exact.world_id.clone(),
                external_mail_id: exact.external_mail_id.clone(),
                sent_at: exact.sent_at,
                recipient: exact.recipient.clone(),
                subject: exact.subject.clone(),
                expected_credits: exact.expected_credits,
                expected_cod: exact.expected_cod,
                expected_equipment_ids: exact.expected_equipment_ids.clone(),
                collected: exact.collected,
                observed_at: exact.observed_at,
            })
            .collect()
    }

    /// 无法唯一归属 Sent 的投递；没有外部证据可写，只按本地状态阻塞。
    fn unresolved_blocks(&self) -> Vec<ExternalRecoveryUnresolvedBlock> {
        self.ambiguous_deliveries
            .iter()
            .chain(self.unconfirmed_deliveries.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|index| {
                let expectation = self.expectations.get(*index)?;
                Some(ExternalRecoveryUnresolvedBlock {
                    delivery_id: expectation.delivery_id,
                    fencing_version: expectation.fencing_version,
                    reason: EXTERNAL_RECOVERY_AMBIGUOUS_REASON.to_owned(),
                    candidate_mail_ids: self
                        .observed
                        .get(index)
                        .map(|mids| {
                            mids.iter()
                                .take(MAX_EXTERNAL_RECOVERY_BLOCK_MAIL_IDS)
                                .cloned()
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// 有候选但未进入精确隔离的投递投影；回执必须让操作者看到未决项。
    fn unresolved_projections(&self) -> Vec<Value> {
        self.ambiguous_deliveries
            .iter()
            .chain(self.unconfirmed_deliveries.iter())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|index| {
                let expectation = self.expectations.get(*index)?;
                Some(json!({
                    "delivery_id": expectation.delivery_id,
                    "fencing_version": expectation.fencing_version,
                    "evidence": evidence_label(expectation.evidence),
                    "world": expectation.world_id,
                    "recipient": expectation.recipient,
                    "frozen_subject_known": expectation.subject.is_some(),
                    "candidate_mail_ids": self.observed.get(index)
                        .map(|mids| mids.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default(),
                    "reason": EXTERNAL_RECOVERY_AMBIGUOUS_REASON,
                }))
            })
            .collect()
    }
}

const fn evidence_label(evidence: ExternalRecoverySendEvidence) -> &'static str {
    match evidence {
        ExternalRecoverySendEvidence::Frozen => "frozen_expectation",
        ExternalRecoverySendEvidence::RestoredPayload => "restored_payload",
    }
}

/// Read Mail 摘要按恢复库入站身份核对；**不**用到达时间截断。
/// 读取时间没有持久证据，备份前到达、备份后才处理并读取的邮件也可能丢失本地记录。
fn missing_inbound(
    entries: &[WorkerRecoveryInboundEntry],
    known: &BTreeSet<(String, String)>,
) -> Vec<Value> {
    entries
        .iter()
        .filter(|entry| !known.contains(&(entry.world.clone(), entry.external_mail_id.clone())))
        .map(|entry| {
            json!({
                "world": entry.world,
                "external_mail_id": entry.external_mail_id,
                "sender": entry.sender,
                "subject": entry.subject,
                "received_at": entry.received_at,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use crate::operations::recovery as parent;
    use auction_application::operations::{WorkerRecoveryInboundEntry, WorkerRecoverySentEntry};

    fn expectation(id: u128, subject: &str) -> ExternalRecoveryDeliveryExpectation {
        ExternalRecoveryDeliveryExpectation {
            delivery_id: Uuid::from_u128(id),
            fencing_version: 2,
            evidence: ExternalRecoverySendEvidence::Frozen,
            world_id: "main".to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: Some(subject.to_owned()),
            credits: 0,
            cod: 0,
            equipment_ids: Some(vec!["317632875".to_owned()]),
        }
    }

    /// 恢复库没有冻结尝试：只能按世界、收件人与资产核对，没有可证明的标题。
    fn payload_expectation(id: u128) -> ExternalRecoveryDeliveryExpectation {
        ExternalRecoveryDeliveryExpectation {
            delivery_id: Uuid::from_u128(id),
            fencing_version: 2,
            evidence: ExternalRecoverySendEvidence::RestoredPayload,
            world_id: "main".to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: None,
            credits: 300_000,
            cod: 0,
            equipment_ids: Some(Vec::new()),
        }
    }

    fn entry(mid: &str, subject: &str) -> WorkerRecoverySentEntry {
        WorkerRecoverySentEntry {
            world: "main".to_owned(),
            external_mail_id: mid.to_owned(),
            recipient: "Restia".to_owned(),
            subject: subject.to_owned(),
            sent_at: Utc::now() - TimeDelta::minutes(10),
        }
    }

    fn detail(mid: &str, subject: &str, collected: bool) -> WorkerSentLookupCandidate {
        WorkerSentLookupCandidate {
            external_mail_id: mid.to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: subject.to_owned(),
            sent_at: Some(Utc::now() - TimeDelta::minutes(10)),
            remaining_credits: 0,
            cod_pending: 0,
            remaining_equipment_ids: if collected {
                Vec::new()
            } else {
                vec!["317632875".to_owned()]
            },
            collected,
            observed_at: Utc::now(),
        }
    }

    fn inbound(world: &str, mid: &str, received_at: DateTime<Utc>) -> WorkerRecoveryInboundEntry {
        WorkerRecoveryInboundEntry {
            world: world.to_owned(),
            external_mail_id: mid.to_owned(),
            sender: "Clo16".to_owned(),
            subject: "[Auction] submission".to_owned(),
            received_at,
        }
    }

    #[test]
    fn apply_requires_complete_sent_scan_and_details() {
        assert!(apply_scan_blockers(true, &[], &[], false, false).is_empty());
        assert_eq!(
            apply_scan_blockers(false, &[], &[], false, false),
            vec!["sent_scan_incomplete"]
        );
        assert_eq!(
            apply_scan_blockers(true, &["sent_timeout".to_owned()], &[], false, false),
            vec!["sent_scan_errors"]
        );
        assert_eq!(
            apply_scan_blockers(true, &[], &["detail_timeout".to_owned()], false, false),
            vec!["sent_detail_errors"]
        );
        assert_eq!(
            apply_scan_blockers(true, &[], &[], true, true),
            vec!["sent_detail_unavailable", "pending_claimed_truncated"]
        );
    }

    #[test]
    fn sent_cutoff_keeps_overlapping_minute() {
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:30Z")
            .expect("cutoff")
            .with_timezone(&Utc);
        let mut sent = entry("100", "[Auction] settlement payment 11111111");
        sent.sent_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
            .expect("sent")
            .with_timezone(&Utc);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(&[expectation], cutoff);
        plan.scan_entries(&[sent]);
        assert_eq!(plan.detail_refs.len(), 1, "同一分钟的 Sent 不能被含秒 cutoff 丢弃");
        let mut detail = detail("100", "[Auction] settlement payment 11111111", false);
        detail.sent_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:00Z")
                .expect("detail sent")
                .with_timezone(&Utc),
        );
        plan.apply_details(&[detail]);
        assert!(plan.quarantine_matches().is_empty(), "重叠分钟不能宣称精确匹配");
        assert_eq!(plan.unresolved_blocks().len(), 1, "重叠分钟必须进入未决阻塞");
    }

    #[test]
    fn sent_cutoff_next_minute_is_exact() {
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:00:30Z")
            .expect("cutoff")
            .with_timezone(&Utc);
        let mut sent = entry("100", "[Auction] settlement payment 11111111");
        sent.sent_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T12:01:00Z")
            .expect("sent")
            .with_timezone(&Utc);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(&[expectation], cutoff);
        plan.scan_entries(&[sent]);
        let mut detail = detail("100", "[Auction] settlement payment 11111111", false);
        detail.sent_at = Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-01T12:01:00Z")
                .expect("detail sent")
                .with_timezone(&Utc),
        );
        plan.apply_details(&[detail]);
        assert_eq!(plan.quarantine_matches().len(), 1);
        assert!(plan.unresolved_blocks().is_empty());
    }

    #[test]
    fn plan_requires_unique_list_candidate_and_exact_detail() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[entry("100", "[Auction] settlement payment 11111111")]);
        assert_eq!(plan.detail_refs.len(), 1);
        plan.apply_details(&[detail(
            "100",
            expectation.subject.as_deref().unwrap_or_default(),
            false,
        )]);
        assert_eq!(plan.exact.len(), 1);
        assert_eq!(plan.quarantine_matches()[0].external_mail_id, "100");
    }

    #[test]
    fn duplicate_list_candidates_are_blocked_not_quarantined() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[
            entry("100", "[Auction] settlement payment 11111111"),
            entry("101", "[Auction] settlement payment 11111111"),
        ]);
        assert!(plan.detail_refs.is_empty());
        assert_eq!(plan.ambiguous, 1);
        assert!(plan.quarantine_matches().is_empty());
        // 无法唯一归属 Sent 时不能留在 pending：必须显式阻塞。
        let blocks = plan.unresolved_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].delivery_id, Uuid::from_u128(7));
        assert_eq!(blocks[0].candidate_mail_ids, vec!["100".to_owned(), "101".to_owned()]);
    }

    #[test]
    fn missing_detail_is_unconfirmed_not_exact() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[entry("100", "[Auction] settlement payment 11111111")]);
        plan.apply_details(&[]);
        assert_eq!(plan.unconfirmed, 1);
        assert!(plan.quarantine_matches().is_empty());
        let blocks = plan.unresolved_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
    }

    /// 恢复库没有冻结尝试时仍然必须进入核对；否则备份后已寄出的投递会再次领取。
    #[test]
    fn restored_payload_delivery_is_matched_without_frozen_attempt() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = payload_expectation(8);
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        assert_eq!(plan.restored_payload_count, 1);
        assert_eq!(plan.frozen_count, 0);
        plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
        assert_eq!(plan.detail_refs.len(), 1);
        let mut payout = detail("200", "[Auction] settlement seller payout", false);
        payout.remaining_credits = 300_000;
        payout.remaining_equipment_ids = Vec::new();
        plan.apply_details(&[payout]);
        assert_eq!(plan.exact.len(), 1);
        assert_eq!(plan.quarantine_matches()[0].expected_equipment_ids, Some(Vec::new()));
    }

    /// pending 默认 fence=0；隔离证据表要求 fence>=1，因此精确匹配也必须走未决阻塞。
    #[test]
    fn pending_fence_zero_exact_match_is_blocked_not_quarantined() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let mut expectation = payload_expectation(10);
        expectation.fencing_version = 0;
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
        let mut payout = detail("200", "[Auction] settlement seller payout", false);
        payout.remaining_credits = 300_000;
        payout.remaining_equipment_ids = Vec::new();
        plan.apply_details(&[payout]);
        assert!(plan.quarantine_matches().is_empty());
        let blocks = plan.unresolved_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
        assert_eq!(blocks[0].fencing_version, 0);
        assert_eq!(blocks[0].candidate_mail_ids, vec!["200".to_owned()]);
    }

    /// payload 重建没有标题；已领取详情无法归因，必须进入未决阻塞而不是留在 pending。
    #[test]
    fn collected_restored_payload_detail_is_blocked() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = payload_expectation(11);
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
        plan.apply_details(&[detail("200", "[Auction] settlement seller payout", true)]);
        assert!(plan.quarantine_matches().is_empty());
        let blocks = plan.unresolved_blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].delivery_id, expectation.delivery_id);
        assert_eq!(blocks[0].candidate_mail_ids, vec!["200".to_owned()]);
    }

    /// 同一 Sent MID 不能证明两条投递；两者都进入未决阻塞而不是隔离。
    #[test]
    fn shared_sent_mid_blocks_both_deliveries() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let first = payload_expectation(8);
        let second = payload_expectation(9);
        let mut plan = SentMatchPlan::new(&[first, second], cutoff);
        plan.scan_entries(&[entry("200", "[Auction] settlement seller payout")]);
        let mut payout = detail("200", "[Auction] settlement seller payout", false);
        payout.remaining_credits = 300_000;
        payout.remaining_equipment_ids = Vec::new();
        plan.apply_details(&[payout]);
        assert!(plan.quarantine_matches().is_empty());
        let mut blocked =
            plan.unresolved_blocks().into_iter().map(|block| block.delivery_id).collect::<Vec<_>>();
        blocked.sort_unstable();
        assert_eq!(blocked, vec![Uuid::from_u128(8), Uuid::from_u128(9)]);
    }

    /// Read Mail 没有持久读取时间：备份前到达、备份后才读取的邮件也必须报告缺失。
    #[test]
    fn read_mail_reconciliation_ignores_arrival_time() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let entries = vec![
            inbound("main", "300", cutoff - TimeDelta::hours(5)),
            inbound("main", "301", cutoff + TimeDelta::minutes(1)),
            inbound("isekai", "302", cutoff - TimeDelta::minutes(1)),
        ];
        let known =
            [(String::from("main"), String::from("301"))].into_iter().collect::<BTreeSet<_>>();
        let missing = missing_inbound(&entries, &known);
        let mids = missing
            .iter()
            .map(|entry| entry["external_mail_id"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(mids, vec!["300".to_owned(), "302".to_owned()]);
    }

    #[test]
    fn apply_requires_confirmation_and_actor() {
        assert!(resolve_apply_authorization(false, None, None).unwrap().is_none());
        assert!(resolve_apply_authorization(true, None, Some("ops")).is_err());
        assert!(resolve_apply_authorization(true, Some("WRONG"), Some("ops")).is_err());
        assert!(resolve_apply_authorization(true, Some(APPLY_CONFIRMATION), None).is_err());
        assert_eq!(
            resolve_apply_authorization(true, Some(APPLY_CONFIRMATION), Some("ops"))
                .unwrap()
                .as_deref(),
            Some("ops")
        );
    }
}
