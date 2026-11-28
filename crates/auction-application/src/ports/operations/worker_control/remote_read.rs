//! 经 Worker control 代为执行的外部只读能力。
//! Admin 手工装备读取与 CLI Sent 核查只描述目标；凭据、请求限速、账号熔断与主动暂停
//! 都由 Worker 进程统一执行，Admin/CLI 不再持有第二个限速器。Worker 不可用时调用方得到明确的
//! 能力不可用结果，而不是回退到本地直连。

use std::future::Future;
use std::pin::Pin;

use auction_domain::EquipmentKey;
use auction_time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ports::admin::{AdminIntakeError, AdminIntakeSourceSnapshot};

/// 远端只读请求的稳定超时（秒）；server 中间件与 client 共用同一上界。
/// 单次读取仍受 Worker 自己的 fetch 预算约束，这里只防止悬挂。
pub const WORKER_REMOTE_READ_TIMEOUT_SECONDS: u64 = 180;

/// 恢复核对的总超时（秒）；多页列表与多个论坛主题共用一次请求。
pub const WORKER_RECOVERY_SCAN_TIMEOUT_SECONDS: u64 = 900;

/// 远端只读的稳定错误分类；不携带远端正文、Cookie 或凭据。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerRemoteReadError {
    /// 主动暂停外部访问；命令未发往远端。
    Paused,
    /// 恢复后核对要求先暂停外部访问；未暂停时不得扫描远端。
    NotPaused,
    /// Worker control 未启用、socket 不存在或对端明确不可用。
    Unavailable,
    /// 超过有界超时。
    Timeout,
    /// 对端协议版本或响应形状不受支持。
    Incompatible,
    /// 目标或参数无效。
    Invalid,
}

impl WorkerRemoteReadError {
    /// 返回稳定名称，用于诊断与 CLI 输出。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Paused => "external_access_paused",
            Self::NotPaused => "external_access_not_paused",
            Self::Unavailable => "worker_control_unavailable",
            Self::Timeout => "worker_control_timeout",
            Self::Incompatible => "worker_control_incompatible",
            Self::Invalid => "invalid_remote_read_request",
        }
    }
}

/// 远端只读的异步结果。
pub type WorkerRemoteReadFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WorkerRemoteReadError>> + Send + 'a>>;

/// Admin 装备 URL 读取请求；只描述世界与目标链接。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEquipmentReadRequest {
    pub world_id: String,
    pub equipment_url: String,
}

impl WorkerEquipmentReadRequest {
    /// 世界闭集与 URL 长度边界；与 control 请求体上限一起保证有界。
    pub fn validate(&self) -> bool {
        matches!(self.world_id.as_str(), "main" | "isekai")
            && !self.equipment_url.is_empty()
            && self.equipment_url.len() <= 512
    }
}

/// Worker 解析后的装备事实；`key` 是公开装备链接中的公示数据，只在同组 peer 的
/// Unix socket 上传输，不进入日志、审计或普通 DTO。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerEquipmentReadResponse {
    pub adapter: String,
    pub source_fingerprint: Vec<u8>,
    pub external_equipment_id: String,
    pub name: String,
    pub category: String,
    pub level: Option<i32>,
    pub tradeable: bool,
    pub attributes: Value,
    pub collected_at: Timestamp,
    pub key: String,
}

impl std::fmt::Debug for WorkerEquipmentReadResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerEquipmentReadResponse")
            .field("adapter", &self.adapter)
            .field("source_fingerprint", &self.source_fingerprint)
            .field("external_equipment_id", &self.external_equipment_id)
            .field("name", &self.name)
            .field("category", &self.category)
            .field("level", &self.level)
            .field("tradeable", &self.tradeable)
            .field("collected_at", &self.collected_at)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl WorkerEquipmentReadResponse {
    /// 从 Worker 侧解析结果建立传输投影。
    pub fn from_snapshot(
        snapshot: AdminIntakeSourceSnapshot,
    ) -> Result<Self, WorkerRemoteReadError> {
        let key = snapshot
            .key
            .with_bytes(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .ok()
            .ok_or(WorkerRemoteReadError::Invalid)?;
        Ok(Self {
            adapter: snapshot.adapter,
            source_fingerprint: snapshot.source_fingerprint.to_vec(),
            external_equipment_id: snapshot.external_equipment_id,
            name: snapshot.name,
            category: snapshot.category,
            level: snapshot.level,
            tradeable: snapshot.tradeable,
            attributes: snapshot.attributes,
            collected_at: snapshot.collected_at,
            key,
        })
    }

    /// 还原为 Admin 录入用例所需的来源快照；所有字段再次校验。
    pub fn into_snapshot(self) -> Result<AdminIntakeSourceSnapshot, WorkerRemoteReadError> {
        let source_fingerprint: [u8; 32] = self
            .source_fingerprint
            .as_slice()
            .try_into()
            .ok()
            .ok_or(WorkerRemoteReadError::Incompatible)?;
        let key = EquipmentKey::new(&self.key).ok().ok_or(WorkerRemoteReadError::Incompatible)?;
        Ok(AdminIntakeSourceSnapshot {
            adapter: self.adapter,
            source_fingerprint,
            external_equipment_id: self.external_equipment_id,
            name: self.name,
            category: self.category,
            level: self.level,
            tradeable: self.tradeable,
            attributes: self.attributes,
            collected_at: self.collected_at,
            key,
        })
    }
}

/// CLI Sent 核查请求；目标与标题由调用方从本地数据库取得。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupRequest {
    pub world: String,
    pub recipient: String,
    pub subject: String,
    pub external_equipment_id: String,
}

impl WorkerSentLookupRequest {
    pub fn validate(&self) -> bool {
        matches!(self.world.as_str(), "main" | "isekai")
            && auction_domain::valid_text(&self.recipient, 256)
            && auction_domain::valid_text(&self.subject, 512)
            && !self.external_equipment_id.is_empty()
            && self.external_equipment_id.len() <= 32
            && self.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
    }
}

/// 单个 world + MID Sent 候选的安全快照；Debug 不复述收件人与标题。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupCandidate {
    pub world: String,
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: Option<Timestamp>,
    pub remaining_credits: i64,
    pub cod_pending: i64,
    pub remaining_equipment_ids: Vec<String>,
    pub collected: bool,
    pub observed_at: Timestamp,
}

impl WorkerSentLookupCandidate {
    /// 校验远端 Sent 详情投影的全部身份、正文与资产字段。
    ///
    /// 恢复核对用这些值判断“未证明发送”是否仍可能已在远端出现；字段口径必须与本地
    /// `DeliverySendExpectation`/`ManualSentEvidence` 相同，否则损坏或越界的详情会被下游
    /// 当作“不是本投递”，从而放行失败或重试，可能重复外部写入（不变量 4、6）。
    pub fn validate(&self) -> bool {
        matches!(self.world.as_str(), "main" | "isekai")
            && auction_domain::valid_text(&self.external_mail_id, 64)
            && crate::ports::admin::valid_mail_player_name(&self.recipient)
            && auction_domain::valid_text(&self.subject, 512)
            && self.sent_at.is_none_or(|value| value.timestamp_nanos_opt().is_some())
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.remaining_credits)
            && (0..=crate::ports::delivery::MAX_DELIVERY_COD).contains(&self.cod_pending)
            && self.remaining_equipment_ids.len() <= MAX_SENT_DETAIL_ATTACHMENTS
            && self.remaining_equipment_ids.iter().all(|id| valid_observed_equipment_id(id))
            && self.observed_at.timestamp_nanos_opt().is_some()
    }
}

impl std::fmt::Debug for WorkerSentLookupCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerSentLookupCandidate")
            .field("external_mail_id", &self.external_mail_id)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("sent_at", &self.sent_at)
            .field("remaining_credits", &self.remaining_credits)
            .field("cod_pending", &self.cod_pending)
            .field("remaining_equipment_ids", &self.remaining_equipment_ids)
            .field("collected", &self.collected)
            .field("observed_at", &self.observed_at)
            .field("world", &self.world)
            .finish()
    }
}

/// Sent 核查的有界结果；每个候选都绑定请求 world，`status` 使用既有闭集名称。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerSentLookupResponse {
    pub status: String,
    pub scan_complete: bool,
    pub pages_scanned: u32,
    pub observed_at: Timestamp,
    pub candidates: Vec<WorkerSentLookupCandidate>,
}
impl WorkerSentLookupResponse {
    /// 只接受属于请求世界、字段完整且条目有界的 Sent 核查结果。
    ///
    /// 控制面响应是不可信输入：`status` 只允许 Worker 的闭集标签（生产者的
    /// `MoogleMailSentLookupStatus` 名称；application 不能依赖 integrations，故在此列出同一
    /// 闭集），未知值直接拒绝，避免任意文本进入 CLI 证据；未通过完整校验的详情也不能进入
    /// 恢复核对，否则越界值会放大内存，或让已发送的邮件被当作不存在并放行失败/重试
    /// （不变量 4、6）。
    pub fn validate_for_world(&self, world: &str) -> bool {
        matches!(world, "main" | "isekai")
            && matches!(
                self.status.as_str(),
                "Unique" | "CollectedUnproven" | "Multiple" | "NotObserved" | "Incomplete"
            )
            && self.observed_at.timestamp_nanos_opt().is_some()
            && self.candidates.len() <= MAX_RECOVERY_SCAN_ENTRIES
            && self
                .candidates
                .iter()
                .all(|candidate| candidate.validate() && candidate.world == world)
    }
}

/// 单封 Sent 详情允许的剩余附件数上限；与发送冻结装备列表上限一致，避免控制面响应放大内存。
const MAX_SENT_DETAIL_ATTACHMENTS: usize = 1_000;

/// 远端 Sent 详情中的附件编号：允许页面保留前导零，但必须是至少一位非零的十进制文本。
/// 冻结的发送预期另用更严格的规范正十进制比较，因此带零前缀的观察只会判为不匹配，不会被当成另一封邮件。
fn valid_observed_equipment_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

/// 恢复后核对：单轮扫描的页数上限与配置校验上限一致。
pub const MAX_RECOVERY_SCAN_PAGES: u32 = 100;
/// 单轮最多确认的 Sent 详情数；每个候选需要一次详情读取，必须有界。
pub const MAX_RECOVERY_SENT_DETAILS: usize = 32;
/// 单轮最多读取的论坛主题数；每个主题完整读取一次。
pub const MAX_RECOVERY_FORUM_THREADS: usize = 20;
/// 响应中每类摘要条目的硬上限；超过时截断并在响应中标记不完整。
pub const MAX_RECOVERY_SCAN_ENTRIES: usize = 2_000;

/// 恢复核对中的世界 + 远端邮件 ID；只描述目标，不含正文或凭据。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryMailRef {
    pub world: String,
    pub external_mail_id: String,
}

impl WorkerRecoveryMailRef {
    pub fn validate(&self) -> bool {
        matches!(self.world.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.external_mail_id, 64)
    }
}

/// 恢复核对要读取的已发布论坛主题；`after_post_number` 是恢复库已处理的最高楼层。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryForumTarget {
    pub world: String,
    pub thread_id: String,
    pub after_post_number: u32,
}

impl WorkerRecoveryForumTarget {
    pub fn validate(&self) -> bool {
        matches!(self.world.as_str(), "main" | "isekai")
            && auction_domain::valid_positive_decimal(&self.thread_id, 32)
            && self.after_post_number >= 1
    }
}

/// 单轮恢复扫描请求；`include_*` 允许第二阶段只读取指定 world + MID 的详情。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryScanRequest {
    /// 备份时刻；Sent 只报告该时刻之后的外部事实。Read Mail 的列表时间列是原始到达时间，
    /// 不能代表本地处理时间，因此 Read Mail 始终扫描到末页并与恢复库身份核对；是否完整由
    /// `read_mail_complete` 表示。
    pub cutoff_at: Timestamp,
    pub maximum_pages: u32,
    /// 需要扫描 Sent 列表的世界；每个世界各一次列表枚举。
    pub sent_worlds: Vec<String>,
    /// 需要扫描 Read Mail 列表的世界；每个世界各一次列表枚举。
    pub read_mail_worlds: Vec<String>,
    pub include_sent: bool,
    pub include_read_mail: bool,
    pub include_forum: bool,
    /// 需要详情确认的 Sent MID；只允许第二阶段传入。
    pub sent_details: Vec<WorkerRecoveryMailRef>,
    pub forum_threads: Vec<WorkerRecoveryForumTarget>,
}

impl WorkerRecoveryScanRequest {
    pub fn validate(&self) -> bool {
        let valid_worlds = |worlds: &[String]| {
            worlds.len() <= 2
                && worlds.iter().all(|world| matches!(world.as_str(), "main" | "isekai"))
                && worlds
                    .iter()
                    .enumerate()
                    .all(|(index, world)| worlds[..index].iter().all(|previous| previous != world))
        };
        self.cutoff_at.timestamp_nanos_opt().is_some()
            && (1..=MAX_RECOVERY_SCAN_PAGES).contains(&self.maximum_pages)
            && valid_worlds(&self.sent_worlds)
            && valid_worlds(&self.read_mail_worlds)
            && (self.include_sent
                || self.include_read_mail
                || self.include_forum
                || !self.sent_details.is_empty())
            && self.sent_details.len() <= MAX_RECOVERY_SENT_DETAILS
            && self.sent_details.iter().all(WorkerRecoveryMailRef::validate)
            && self.sent_details.iter().enumerate().all(|(index, detail)| {
                self.sent_details[..index].iter().all(|previous| previous != detail)
            })
            && self.forum_threads.len() <= MAX_RECOVERY_FORUM_THREADS
            && self.forum_threads.iter().all(WorkerRecoveryForumTarget::validate)
            && self.forum_threads.iter().enumerate().all(|(index, target)| {
                self.forum_threads[..index].iter().all(|previous| {
                    previous.world != target.world || previous.thread_id != target.thread_id
                })
            })
    }
}

/// Sent 列表摘要的安全投影；列表字段可能被远端截断，不能作为最终身份证明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoverySentEntry {
    pub world: String,
    pub external_mail_id: String,
    pub recipient: String,
    pub subject: String,
    pub sent_at: Timestamp,
}

/// Read Mail 列表摘要的安全投影；不含正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryInboundEntry {
    pub world: String,
    pub external_mail_id: String,
    pub sender: String,
    pub subject: String,
    pub received_at: Timestamp,
}

/// 恢复库可能没有记录的论坛楼层；只含稳定 ID 与楼层号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryForumPost {
    pub world: String,
    pub thread_id: String,
    pub post_id: String,
    pub post_number: u32,
}

/// 扫描结果；`errors` 只含稳定分类码。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerRecoveryScanResponse {
    /// `complete`、`truncated` 或 `partial`。
    pub status: String,
    pub observed_at: Timestamp,
    /// 执行时暂停仍生效；false 表示调用期间暂停被解除，结果不能用于隔离。
    pub paused: bool,
    pub sent_complete: bool,
    pub read_mail_complete: bool,
    pub pages_scanned: u32,
    pub errors: Vec<String>,
    pub sent: Vec<WorkerRecoverySentEntry>,
    pub read_mail: Vec<WorkerRecoveryInboundEntry>,
    pub sent_details: Vec<WorkerSentLookupCandidate>,
    pub forum: Vec<WorkerRecoveryForumPost>,
}

impl WorkerRecoveryScanResponse {
    /// 稳定形状校验；任何不可验证的响应都按协议不兼容处理。
    pub fn validate(&self) -> bool {
        matches!(self.status.as_str(), "complete" | "truncated" | "partial")
            && self.observed_at.timestamp_nanos_opt().is_some()
            && self.errors.len() <= 16
            && self.errors.iter().all(|code| {
                !code.is_empty()
                    && code.len() <= 64
                    && code.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            })
            && self.sent.len() <= MAX_RECOVERY_SCAN_ENTRIES
            && self.read_mail.len() <= MAX_RECOVERY_SCAN_ENTRIES
            && self.sent_details.len() <= MAX_RECOVERY_SENT_DETAILS
            && self.forum.len() <= MAX_RECOVERY_FORUM_THREADS * 1_000
            && self.sent.iter().all(|entry| {
                matches!(entry.world.as_str(), "main" | "isekai")
                    && auction_domain::valid_positive_decimal(&entry.external_mail_id, 64)
                    && !entry.recipient.is_empty()
                    && !entry.subject.is_empty()
            })
            && self.read_mail.iter().all(|entry| {
                matches!(entry.world.as_str(), "main" | "isekai")
                    && auction_domain::valid_positive_decimal(&entry.external_mail_id, 64)
                    && !entry.sender.is_empty()
                    && !entry.subject.is_empty()
            })
            && self.sent_details.iter().all(WorkerSentLookupCandidate::validate)
            && self.forum.iter().all(|post| {
                matches!(post.world.as_str(), "main" | "isekai")
                    && auction_domain::valid_positive_decimal(&post.thread_id, 32)
                    && auction_domain::valid_positive_decimal(&post.post_id, 32)
                    && post.post_number >= 1
            })
    }
}

/// Admin 装备 URL 的远端来源读取（Worker 侧真实执行、Admin/CLI 侧走 control client）。
pub trait WorkerRemoteReadPort: Send + Sync {
    fn read_equipment(
        &self,
        request: WorkerEquipmentReadRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse>;

    fn lookup_sent(
        &self,
        request: WorkerSentLookupRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse>;

    /// 暂停状态下的一次有界只读恢复扫描。
    /// 默认实现明确返回能力不可用，装配根未接入时不会静默跳过。
    fn recovery_scan(
        &self,
        _request: WorkerRecoveryScanRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerRecoveryScanResponse> {
        boxed(async { Err(WorkerRemoteReadError::Unavailable) })
    }
}

impl From<WorkerRemoteReadError> for AdminIntakeError {
    fn from(error: WorkerRemoteReadError) -> Self {
        match error {
            WorkerRemoteReadError::Paused => Self::ExternalAccessPaused,
            WorkerRemoteReadError::Invalid => Self::InvalidCommand,
            WorkerRemoteReadError::NotPaused
            | WorkerRemoteReadError::Unavailable
            | WorkerRemoteReadError::Timeout
            | WorkerRemoteReadError::Incompatible => Self::SourceUnavailable,
        }
    }
}

/// 供 `ApplicationFuture` 无关的调用方复用的装箱辅助。
pub(crate) fn boxed<'a, T: Send + 'a>(
    future: impl Future<Output = Result<T, WorkerRemoteReadError>> + Send + 'a,
) -> WorkerRemoteReadFuture<'a, T> {
    Box::pin(future)
}

/// Worker 侧控制不可用时 Admin 端使用的立即失败端口；不回退到本地直连。
pub struct UnavailableWorkerRemoteRead;

impl WorkerRemoteReadPort for UnavailableWorkerRemoteRead {
    fn read_equipment(
        &self,
        _request: WorkerEquipmentReadRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerEquipmentReadResponse> {
        boxed(async { Err(WorkerRemoteReadError::Unavailable) })
    }

    fn lookup_sent(
        &self,
        _request: WorkerSentLookupRequest,
    ) -> WorkerRemoteReadFuture<'_, WorkerSentLookupResponse> {
        boxed(async { Err(WorkerRemoteReadError::Unavailable) })
    }
}

#[cfg(test)]
mod tests {
    use auction_time::TimeDelta;
    use serde_json::json;

    use super::*;

    fn snapshot() -> AdminIntakeSourceSnapshot {
        AdminIntakeSourceSnapshot {
            adapter: "hentaiverse-equipment-v1".to_owned(),
            source_fingerprint: [7; 32],
            external_equipment_id: "300753957".to_owned(),
            name: "Legendary Radiant Phase Shoes".to_owned(),
            category: "Shoes".to_owned(),
            level: Some(500),
            tradeable: true,
            attributes: json!({"Primary Attributes / Strength": {"value": "+10.00"}}),
            collected_at: Timestamp::now() - TimeDelta::seconds(30),
            key: EquipmentKey::new("0123456789").expect("test key is ten hex characters"),
        }
    }

    #[test]
    fn equipment_projection_round_trips_without_leaking_key_in_debug() {
        let response =
            WorkerEquipmentReadResponse::from_snapshot(snapshot()).expect("key is exposable");
        assert_eq!(response.key, "0123456789");
        let restored = response.clone().into_snapshot().expect("projection is valid");
        assert_eq!(restored.source_fingerprint, [7; 32]);
        assert_eq!(restored.external_equipment_id, "300753957");
        // 传输投影的 Debug 只暴露公开结构，key 不从 Debug 输出。
        let debug = format!("{response:?}");
        assert!(!debug.contains("0123456789"));
    }

    #[test]
    fn equipment_projection_rejects_corrupt_fingerprint_or_key() {
        let mut response =
            WorkerEquipmentReadResponse::from_snapshot(snapshot()).expect("key is exposable");
        response.source_fingerprint = vec![1; 31];
        assert_eq!(
            response.clone().into_snapshot().err(),
            Some(WorkerRemoteReadError::Incompatible)
        );
        response.source_fingerprint = vec![1; 32];
        response.key = "zzzzzzzzzz".to_owned();
        assert_eq!(response.into_snapshot().err(), Some(WorkerRemoteReadError::Incompatible));
    }

    #[test]
    fn request_validation_covers_world_bounds_and_url_length() {
        let valid = WorkerEquipmentReadRequest {
            world_id: "main".to_owned(),
            equipment_url: "https://hentaiverse.org/equip/300753957/0123456789".to_owned(),
        };
        assert!(valid.validate());
        assert!(!WorkerEquipmentReadRequest { world_id: "shared".to_owned(), ..valid }.validate());
        assert!(
            !WorkerEquipmentReadRequest {
                world_id: "main".to_owned(),
                equipment_url: "x".repeat(513)
            }
            .validate()
        );

        let sent = WorkerSentLookupRequest {
            world: "isekai".to_owned(),
            recipient: "Sample·Fixture".to_owned(),
            subject: "Withdrawn return".to_owned(),
            external_equipment_id: "317632875".to_owned(),
        };
        assert!(sent.validate());
        assert!(
            !WorkerSentLookupRequest { external_equipment_id: "12a".to_owned(), ..sent.clone() }
                .validate()
        );
        assert!(!WorkerSentLookupRequest { recipient: String::new(), ..sent }.validate());
    }

    fn sent_lookup_candidate(world: &str) -> WorkerSentLookupCandidate {
        WorkerSentLookupCandidate {
            world: world.to_owned(),
            external_mail_id: "3125345".to_owned(),
            recipient: "Sample·Fixture".to_owned(),
            subject: "Withdrawn return".to_owned(),
            sent_at: Some(Timestamp::now() - TimeDelta::seconds(60)),
            remaining_credits: 0,
            cod_pending: 0,
            remaining_equipment_ids: vec!["317632875".to_owned()],
            collected: false,
            observed_at: Timestamp::now(),
        }
    }

    fn sent_lookup_response(
        status: &str,
        candidates: Vec<WorkerSentLookupCandidate>,
    ) -> WorkerSentLookupResponse {
        WorkerSentLookupResponse {
            status: status.to_owned(),
            scan_complete: true,
            pages_scanned: 1,
            observed_at: Timestamp::now(),
            candidates,
        }
    }

    /// `status` 只接受 Worker 闭集标签；候选必须属于请求世界且逐字段有效。
    #[test]
    fn sent_lookup_response_enforces_status_and_world_closed_sets() {
        let valid = sent_lookup_response("NotObserved", Vec::new());
        assert!(valid.validate_for_world("main"));
        assert!(!sent_lookup_response("unique", Vec::new()).validate_for_world("main"));
        assert!(!sent_lookup_response("Unknown", Vec::new()).validate_for_world("main"));

        let foreign = sent_lookup_response("Unique", vec![sent_lookup_candidate("main")]);
        assert!(foreign.validate_for_world("main"));
        assert!(!foreign.validate_for_world("isekai"), "候选 world 必须等于请求 world");
    }

    #[test]
    fn paused_maps_to_admin_intake_external_access_paused() {
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Paused),
            AdminIntakeError::ExternalAccessPaused
        );
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Unavailable),
            AdminIntakeError::SourceUnavailable
        );
        assert_eq!(
            AdminIntakeError::from(WorkerRemoteReadError::Invalid),
            AdminIntakeError::InvalidCommand
        );
    }
}
