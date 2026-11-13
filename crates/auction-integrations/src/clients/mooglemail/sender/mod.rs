//! MoogleMail 分步写入与发送后 Sent 完成证明。
//!
//! 发送编排、扫描水位与证据映射分别位于 `send`、`scan`、`evidence` 子模块；
//! 这里只保留共享类型和构造入口。

use auction_application::mail::{MailScanCheckpoint, MailScanCheckpointStore, MailScanObserver};

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

mod baseline;
mod equipment;
mod evidence;
mod maintenance;
mod proof;
mod scan;
mod send;
#[cfg(test)]
mod tests;

pub(crate) use baseline::MoogleMailSentBaseline;

pub use maintenance::{
    MoogleMailComposeSnapshot, MoogleMailDiscardOutcome, MoogleMailSentListEntry,
    MoogleMailSentObservation,
};

use evidence::{preserve_write_unknown, staged_proof_error, write_error, write_unknown};

// 子模块沿用 `use super::*` 的既有约定，这里保留它们共享的客户端前缀。
use super::{
    MoogleMailClient, MoogleMailCookie, MoogleMailError, MoogleMailErrorCategory, MoogleMailForm,
    MoogleMailInventoryCategory, MoogleMailMessageFields, MoogleMailOperation, MoogleMailParser,
    MoogleMailWorld, protocol::valid_decimal_id,
};

const MAX_SEND_ATTACHMENTS: usize = 1000;

/// 一封待发送 MoogleMail 的业务无关输入。
#[derive(Clone, PartialEq, Eq)]
pub struct MoogleMailSendRequest {
    pub world: MoogleMailWorld,
    pub message: MoogleMailMessageFields,
    pub credits: i64,
    /// 要设置的 CoD 数量；零表示不设置 CoD。
    pub cod: i64,
    pub equipment_ids: Vec<String>,
    /// Armory 读取提示；只减少无效页面读取，未命中或无法识别时回退完整七类扫描。
    pub category: Option<MoogleMailInventoryCategory>,
}

impl MoogleMailSendRequest {
    pub fn validate(&self) -> bool {
        self.message.validate()
            && self.credits >= 0
            && self.cod >= 0
            && self.equipment_ids.len() <= MAX_SEND_ATTACHMENTS
            && self.equipment_ids.iter().all(|id| valid_decimal_id(id))
            && self.equipment_ids.iter().collect::<HashSet<_>>().len() == self.equipment_ids.len()
            && (self.cod == 0 || self.credits > 0 || !self.equipment_ids.is_empty())
    }
}

impl fmt::Debug for MoogleMailSendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoogleMailSendRequest")
            .field("world", &self.world)
            .field("message", &"[REDACTED]")
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("equipment_count", &self.equipment_ids.len())
            .field("category", &self.category)
            .finish()
    }
}

/// 已经由新增 Sent 邮件的身份、正文和精确资产证明的寄送结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailSendReceipt {
    /// 通过发送前后 Sent 差异核对得到的真实远端邮件 ID。
    pub external_mail_id: String,
}

/// 最终 send POST 的结构化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoogleMailSendOutcome {
    /// 新增 Sent 邮件已通过完整内容核验。
    Sent(MoogleMailSendReceipt),
    /// compose 初始页无法证明为干净状态；`dirty` 表示页面可解析但仍有草稿资产。
    PreflightUnproven { dirty: bool },
    /// 远端可见文字明确报告收件人不存在。
    RecipientNotFound { returned_mail_id: Option<String> },
    /// 远端明确拒绝自寄；已经暂存的资产仍须人工核对，不能当作交付成功。
    RecipientIsSender { assets_staged: bool },
    /// 远端明确拒绝发送；附件已经分步放入 compose 时不能自动重试。
    Rejected { assets_staged: bool },
}

/// 负责把一封 typed message 编排为受证明的多步 compose 写入。
#[derive(Clone)]
pub struct MoogleMailSender {
    client: MoogleMailClient,
    parser: MoogleMailParser,
    verification_pages: usize,
    verification_budget: Duration,
    sent_baseline_mode: auction_application::sent_baseline::SentBaselineMode,
    sent_baseline_observer: Arc<dyn auction_application::sent_baseline::SentBaselineObserver>,
    /// 最近一次已完整证明的 Sent 摘要；只作排除集，缺失条目最多增加候选详情读取。
    sent_baselines: Arc<Mutex<HashMap<MoogleMailWorld, CachedSentBaseline>>>,
    /// Sent 扫描水位；只缩短下一轮前缀，重启即回退完整扫描。
    sent_scans: Arc<Mutex<HashMap<MoogleMailWorld, CachedSentScan>>>,
    /// Read Mail 观测水位；独立于投递游标，只用于维护观察计数。
    read_scans: Arc<Mutex<HashMap<MoogleMailWorld, CachedReadScan>>>,
    /// 可选持久检查点；只缩短重启后的前缀扫描，读取失败仍回退完整扫描。
    scan_checkpoints: Option<Arc<dyn MailScanCheckpointStore>>,
    scan_observer: Arc<dyn MailScanObserver>,
}

struct CachedSentScan {
    credential: uuid::Uuid,
    scope: super::client::MoogleMailSentScope,
    checkpoint: MailScanCheckpoint,
}

struct CachedReadScan {
    credential: uuid::Uuid,
    checkpoint: MailScanCheckpoint,
}

struct CachedSentBaseline {
    credential: uuid::Uuid,
    scope: super::client::MoogleMailSentScope,
    seen: HashSet<String>,
    captured: Instant,
}

impl MoogleMailSender {
    /// 使用共享 MoogleMail client 构造发送编排器。
    pub fn new(client: MoogleMailClient) -> Self {
        Self {
            client,
            parser: MoogleMailParser,
            verification_pages: 100,
            verification_budget: Duration::from_secs(30),
            sent_baseline_mode: auction_application::sent_baseline::SentBaselineMode::Shadow,
            sent_baseline_observer: auction_application::sent_baseline::noop_sent_baseline_observer(
            ),
            sent_baselines: Arc::new(Mutex::new(HashMap::new())),
            sent_scans: Arc::new(Mutex::new(HashMap::new())),
            read_scans: Arc::new(Mutex::new(HashMap::new())),
            scan_checkpoints: None,
            scan_observer: auction_application::mail::noop_mail_scan_observer(),
        }
    }

    /// 设置发送前后 Sent 核对的页数和总预算。
    pub fn with_send_verification(
        mut self,
        maximum_pages: usize,
        budget: Duration,
    ) -> Result<Self, MoogleMailError> {
        if maximum_pages == 0 || maximum_pages > 100 || budget.is_zero() {
            return Err(MoogleMailError::new(
                MoogleMailOperation::Send,
                MoogleMailWorld::Main,
                MoogleMailErrorCategory::Configuration,
                false,
            ));
        }
        self.verification_pages = maximum_pages;
        self.verification_budget = budget;
        Ok(self)
    }
}
