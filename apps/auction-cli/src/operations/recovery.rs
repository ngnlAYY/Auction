//! W10.9 恢复后外部核对：暂停 Worker 下只读扫描 Sent / Read Mail / 论坛新楼层，
//! 把恢复库中仍为 pending/claimed 且与 Sent 精确匹配的投递隔离为人工对账。
//!
//! 本命令只读取远端列表、为候选 MID 读取详情，并在显式 `--apply` 时写入隔离事实；
//! 绝不重发、领取、标记已读或改写资金与结算事实。Worker 未暂停、能力不可用或证据不完整时
//! 明确失败或把回执标记为不完整，不能用“未观察到”推断“未发送”。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use anyhow::Context;
use auction_application::{
    EXTERNAL_RECOVERY_RECEIPT_FORMAT, ExternalRecoveryDeliveryExpectation,
    ExternalRecoveryQuarantineMatch, ExternalRecoveryStore, SentDetailMatch,
    WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS, WorkerRecoveryMailRef, WorkerRecoveryScanRequest,
    WorkerRecoveryScanResponse, WorkerRemoteReadError, WorkerRemoteReadPort,
    WorkerSentLookupCandidate, sent_detail_match, sent_list_projection_may_match,
};
use auction_config::AppConfig;
use auction_storage::DatabaseStore;
use chrono::{DateTime, TimeDelta, Utc};
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
                .map(|thread| auction_application::WorkerRecoveryForumTarget {
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
        .known_inbound_mail_ids(read_mail_candidates.clone())
        .await
        .context("读取恢复库已知入站邮件失败")?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let known_lookup = known.clone();
    let missing_inbound = first
        .read_mail
        .iter()
        .filter(|entry| {
            !known_lookup.contains(&(entry.world.clone(), entry.external_mail_id.clone()))
        })
        .map(|entry| {
            json!({
                "world": entry.world,
                "external_mail_id": entry.external_mail_id,
                "sender": entry.sender,
                "subject": entry.subject,
                "received_at": entry.received_at,
            })
        })
        .collect::<Vec<_>>();

    let quarantine_matches = plan.quarantine_matches();
    let mut outcomes = Vec::new();
    if apply && !quarantine_matches.is_empty() {
        let actor = actor.context("--apply 缺少操作者")?;
        let applied = store
            .quarantine_external_sent_matches(scan_id, actor.to_owned(), quarantine_matches)
            .await
            .context("隔离精确匹配投递失败")?;
        outcomes = applied
            .into_iter()
            .map(|outcome| {
                json!({
                    "delivery_id": outcome.delivery_id,
                    "status": outcome.status,
                    "fencing_version": outcome.fencing_version,
                    "reason": outcome.reason,
                })
            })
            .collect();
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
                "world": exact.world_id,
                "external_mail_id": exact.external_mail_id,
                "sent_at": exact.sent_at,
            })).collect::<Vec<_>>(),
            "quarantined": outcomes,
        },
        "read_mail": {
            "scanned": first.read_mail.len(),
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
        "quarantined": outcomes.len(),
        "missing_inbound": receipt["read_mail"]["missing_locally"].as_array().map_or(0, Vec::len),
        "missing_forum_posts": first.forum.len(),
    }))
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
    cutoff_at: Option<DateTime<Utc>>,
    /// 有列表候选但尚未读取详情的投递 → 候选 MID。
    pending_details: Vec<(usize, SentListCandidate)>,
    detail_refs: BTreeSet<WorkerRecoveryMailRef>,
    exact: Vec<ExactDeliveryMatch>,
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
    world_id: String,
    external_mail_id: String,
    sent_at: DateTime<Utc>,
    recipient: String,
    subject: String,
    expected_credits: i64,
    expected_cod: i64,
    expected_equipment_ids: Vec<String>,
    collected: bool,
    observed_at: DateTime<Utc>,
}

impl SentMatchPlan {
    fn new(expectations: &[ExternalRecoveryDeliveryExpectation], cutoff_at: DateTime<Utc>) -> Self {
        Self { expectations: expectations.to_vec(), cutoff_at: Some(cutoff_at), ..Self::default() }
    }

    /// 先用列表投影筛选详情候选；只有唯一候选才进入第二阶段。
    fn scan_entries(&mut self, entries: &[auction_application::WorkerRecoverySentEntry]) {
        let Some(cutoff_at) = self.cutoff_at else { return };
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
            match candidates.as_slice() {
                [] => {}
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
                _ => self.ambiguous += 1,
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
                continue;
            };
            if details.len() != 1 {
                self.ambiguous += 1;
                continue;
            }
            let detail = details[0];
            let Some(sent_at) = detail.sent_at else {
                self.unconfirmed += 1;
                continue;
            };
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
                    matches.push((delivery_index, candidate, detail.clone(), sent_at));
                }
                SentDetailMatch::AssetMismatch => self.asset_mismatch += 1,
                SentDetailMatch::IdentityMismatch | SentDetailMatch::Ambiguous => {
                    self.identity_mismatch += 1;
                }
            }
        }
        // 同一 Sent MID 只能证明一个投递；重复即两者都不隔离。
        let mut mid_counts: BTreeMap<String, usize> = BTreeMap::new();
        for (_, _, detail, _) in &matches {
            *mid_counts.entry(detail.external_mail_id.clone()).or_default() += 1;
        }
        for (delivery_index, candidate, detail, sent_at) in matches {
            if mid_counts.get(&detail.external_mail_id).copied().unwrap_or(0) > 1 {
                self.ambiguous += 1;
                continue;
            }
            let expectation = &self.expectations[delivery_index];
            self.exact.push(ExactDeliveryMatch {
                delivery_id: expectation.delivery_id,
                fencing_version: expectation.fencing_version,
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
}

#[cfg(test)]
mod tests {
    use self::parent::*;
    use super as parent;
    use auction_application::WorkerRecoverySentEntry;

    fn expectation(id: u128, subject: &str) -> ExternalRecoveryDeliveryExpectation {
        ExternalRecoveryDeliveryExpectation {
            delivery_id: Uuid::from_u128(id),
            fencing_version: 2,
            world_id: "main".to_owned(),
            recipient: "Restia﹒Ashdoll".to_owned(),
            subject: subject.to_owned(),
            credits: 0,
            cod: 0,
            equipment_ids: vec!["317632875".to_owned()],
            sending_at: Utc::now() - TimeDelta::minutes(60),
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

    #[test]
    fn plan_requires_unique_list_candidate_and_exact_detail() {
        let cutoff = Utc::now() - TimeDelta::hours(1);
        let expectation = expectation(7, "[Auction] settlement payment 11111111");
        let mut plan = SentMatchPlan::new(std::slice::from_ref(&expectation), cutoff);
        plan.scan_entries(&[entry("100", "[Auction] settlement payment 11111111")]);
        assert_eq!(plan.detail_refs.len(), 1);
        plan.apply_details(&[detail("100", &expectation.subject, false)]);
        assert_eq!(plan.exact.len(), 1);
        assert_eq!(plan.quarantine_matches()[0].external_mail_id, "100");
    }

    #[test]
    fn duplicate_list_candidates_are_ambiguous_and_never_quarantined() {
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
