//! MoogleMail gateway 的配置和安全读取结果模型。

use std::{
    fmt,
    time::{Duration, Instant},
};

use auction_time::Timestamp;

use super::MAX_HARD_PAGES;
use super::reader::BoundConsignmentObservation;
use crate::clients::mooglemail::parser::{
    ConsignmentListRow, MoogleMailMessage, MoogleMailSentSnapshot, MoogleMailSentSummary,
    MoogleMailSummary,
};
use crate::clients::mooglemail::protocol::MoogleMailWorld;

/// MoogleMail 收件箱 gateway 的有界扫描配置。
#[derive(Debug, Clone)]
pub struct MoogleMailGatewayConfig {
    pub maximum_pages: usize,
    /// 发件人 allowlist；授权只使用详情中的完整发件人，列表截断显示名不能替代身份。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许所有完整发件人通过授权。
    pub sender_allowlist_disabled: bool,
    /// 单次 inbox 扫描及详情读取共享的总预算，覆盖限速、凭据失效处理和 HTTP 正文读取。
    pub fetch_budget: Duration,
}

impl Default for MoogleMailGatewayConfig {
    fn default() -> Self {
        Self {
            maximum_pages: MAX_HARD_PAGES,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
            fetch_budget: Duration::from_secs(30),
        }
    }
}

/// 只读 Sent 核对请求；不会执行任何写入动作。
#[derive(Clone)]
pub struct MoogleMailSentLookupRequest {
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    pub external_equipment_id: String,
    pub maximum_pages: usize,
    pub deadline: Instant,
    pub observed_at: Timestamp,
}

impl fmt::Debug for MoogleMailSentLookupRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSentLookupRequest")
            .field("world", &self.world)
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("external_equipment_id", &self.external_equipment_id)
            .field("maximum_pages", &self.maximum_pages)
            .field("deadline", &self.deadline)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

impl MoogleMailSentLookupRequest {
    pub fn validate(&self, configured_pages: usize) -> bool {
        auction_domain::valid_text(&self.recipient, 256)
            && auction_domain::valid_text(&self.subject, 512)
            && !self.external_equipment_id.is_empty()
            && self.external_equipment_id.len() <= 32
            && self.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
            && (self.external_equipment_id.len() == 1
                || self.external_equipment_id.as_bytes()[0] != b'0')
            && (1..=configured_pages).contains(&self.maximum_pages)
            && self.observed_at.timestamp_nanos_opt().is_some()
    }
}

/// Sent 核对结果分类；只有 `Unique` 且候选未领取时才具备可继续人工判断的条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoogleMailSentLookupStatus {
    Unique,
    CollectedUnproven,
    Multiple,
    NotObserved,
    Incomplete,
}

/// Sent 核对的安全结果，不包含 HTML、正文、Cookie 或请求表单。
#[derive(Debug, Clone)]
pub struct MoogleMailSentLookupReport {
    pub status: MoogleMailSentLookupStatus,
    pub candidates: Vec<MoogleMailSentSnapshot>,
    pub pages_scanned: usize,
    pub scan_complete: bool,
    pub observed_at: Timestamp,
}

impl MoogleMailSentLookupStatus {
    /// 结论标签；CLI 证据文件自该版起使用同一名称，保持既有回执字段不变。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unique => "Unique",
            Self::CollectedUnproven => "CollectedUnproven",
            Self::Multiple => "Multiple",
            Self::NotObserved => "NotObserved",
            Self::Incomplete => "Incomplete",
        }
    }
}

/// 一页中的一封邮件：旧消费者与寄售候选两态。
pub enum MoogleMailPageMessage {
    /// 走既有通用解析的旧信。
    Legacy(MoogleMailMessage),
    /// 寄售候选：同一份详情文档的通用解析结果与 4D1 绑定观察。
    ConsignmentCandidate {
        /// 同一详情文档的通用解析结果；本段仍按旧信透传。
        message: MoogleMailMessage,
        /// 4D1 候选。
        candidate: Box<MoogleMailConsignmentCandidate>,
    },
    /// 寄售候选，但严格通用解析失败（如重复原文 EID）：只有 4D1 宽容观察，没有通用解析结果。
    /// 下游不得走旧路由，只能交寄售事务落成待隔离观察。
    UnparsedConsignmentCandidate(Box<MoogleMailConsignmentCandidate>),
}

impl fmt::Debug for MoogleMailPageMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(message) => formatter.debug_tuple("Legacy").field(message).finish(),
            Self::ConsignmentCandidate { candidate, .. } => {
                formatter.debug_tuple("ConsignmentCandidate").field(candidate).finish()
            }
            Self::UnparsedConsignmentCandidate(candidate) => {
                formatter.debug_tuple("UnparsedConsignmentCandidate").field(candidate).finish()
            }
        }
    }
}

impl MoogleMailPageMessage {
    /// 邮件所属 world。
    pub fn world(&self) -> MoogleMailWorld {
        match self {
            Self::Legacy(message) | Self::ConsignmentCandidate { message, .. } => message.world,
            Self::UnparsedConsignmentCandidate(candidate) => candidate.row.world,
        }
    }

    /// 远端邮件身份；游标与完成账本仍只使用该值。
    pub fn external_mail_id(&self) -> &str {
        match self {
            Self::Legacy(message) => &message.external_mail_id,
            Self::ConsignmentCandidate { message, .. } => &message.external_mail_id,
            Self::UnparsedConsignmentCandidate(candidate) => &candidate.row.external_mail_id,
        }
    }

    /// 拆出通用解析结果；寄售候选同样先按旧信透传，严格解析失败的候选没有通用解析结果。
    pub fn into_parts(self) -> (Option<MoogleMailMessage>, Option<MoogleMailConsignmentCandidate>) {
        match self {
            Self::Legacy(message) => (Some(message), None),
            Self::ConsignmentCandidate { message, candidate } => (Some(message), Some(*candidate)),
            Self::UnparsedConsignmentCandidate(candidate) => (None, Some(*candidate)),
        }
    }
}

/// 寄售候选；通用解析结果由旧信透传，4D1 观察供 I2b2 的寄售处理使用。
pub struct MoogleMailConsignmentCandidate {
    /// 列表原始投影（原始分钟与世界标记）。
    pub row: ConsignmentListRow,
    /// 绑定到认证会话的 4D1 观察。
    pub bound: BoundConsignmentObservation,
}

impl fmt::Debug for MoogleMailConsignmentCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailConsignmentCandidate")
            .field("external_mail_id", &self.row.external_mail_id)
            .field("world", &self.row.world)
            .field("bound", &self.bound)
            .finish()
    }
}

/// 已完成摘要/详情交叉校验的一次有界邮件拉取。
#[derive(Debug)]
pub struct MoogleMailPage {
    /// 按最旧到最新顺序返回，便于应用层顺序入库。
    pub messages: Vec<MoogleMailPageMessage>,
    /// 本批最新已观测摘要的 external ID；空批由调用方保留旧 cursor。
    pub next_cursor: Option<String>,
}

/// 恢复核对：`cutoff_at` 之后的 Sent 列表摘要。
/// 列表字段可能被远端截断，只用于筛选详情候选，不是发送或身份证明。
#[derive(Debug, Clone)]
pub struct MoogleMailSentRecoveryScan {
    pub entries: Vec<MoogleMailSentSummary>,
    /// 扫描到的页是否已经覆盖 `cutoff_at` 之后的全部条目。
    pub covers_cutoff: bool,
    pub pages_scanned: usize,
    pub observed_at: Timestamp,
}

/// 恢复核对：Read Mail 全量列表摘要；不含正文。
///
/// 列表时间列是远端原始到达时间，不是本地读取/处理时间，因此这里不做时间截断。
#[derive(Debug, Clone)]
pub struct MoogleMailInboundRecoveryScan {
    pub entries: Vec<MoogleMailSummary>,
    /// 是否枚举到列表末页；只有为 true 才能声称 Read Mail 覆盖完整。
    pub covers_cutoff: bool,
    pub observed_at: Timestamp,
}
