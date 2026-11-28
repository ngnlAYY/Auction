//! 寄售来信的原始观察解析：保留原序、原文 EID、收件人与未判标记。
//!
//! 本模块只产出观察，不决定接受、退回或隔离。重复原文 EID、规范别名歧义、未知附件布局、
//! 列表与详情冲突、资产数或金额不明都进入显式未判字段，避免上层把受控观察误当成空信丢弃。

use std::fmt;

use auction_domain::consignment::{EnvelopeItemKind, RemovedMarker};
use auction_time::Timestamp;

use crate::clients::mooglemail::{MoogleMailEquipmentKey, MoogleMailWorld};

mod observation;
mod parse;
mod receipt;

pub use parse::parse_consignment_detail;
pub use parse::parse_consignment_list_row;
pub use receipt::observe_take_receipt;

/// 寄售原观察解析器版本；真实成对样本签收前固定为未签收。
pub const CONSIGNMENT_PARSER_VERSION: &str = "mooglemail-consignment-parser-v1-unsigned";

/// 收件列表中的一行；保留原始分钟文本和列表显示身份。
#[derive(Clone, PartialEq, Eq)]
pub struct ConsignmentListRow {
    /// 远端 MID。
    pub external_mail_id: String,
    /// 读取世界。
    pub world: MoogleMailWorld,
    /// 列表声明的世界标记。
    pub world_marker: Option<MoogleMailWorld>,
    /// 列表发件人显示名，可能被截断。
    pub sender: String,
    /// 列表主题，可能按 35 字节截断。
    pub subject: String,
    /// 页面原始分钟文本。
    pub received_minute_raw: String,
    /// 由原始分钟文本解释出的 UTC 时刻。
    pub received_at: Timestamp,
}

impl fmt::Debug for ConsignmentListRow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentListRow")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("world_marker", &self.world_marker)
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("received_minute_raw", &self.received_minute_raw)
            .field("received_at", &self.received_at)
            .finish()
    }
}

/// 原始附件观察；保留原文 EID、种类、显示名和短生命周期装备 key。
pub struct ConsignmentAttachmentObservation {
    /// 原序号，从 1 开始。
    pub ordinal: u32,
    /// 附件种类。
    pub kind: EnvelopeItemKind,
    /// 页面原文 EID。
    pub raw_eid: Option<String>,
    /// 去前导零后的规范 EID；原文不是正十进制时为空。
    pub canonical_eid: Option<String>,
    /// Credits 金额；装备为空。
    pub amount: Option<u64>,
    /// 附件显示名；用于按 `Removed attachment:` 逐项配对。
    pub name: String,
    /// 读取装备页所需的短生命周期 key；不在任何格式化中输出。
    pub key: Option<MoogleMailEquipmentKey>,
}

impl fmt::Debug for ConsignmentAttachmentObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentAttachmentObservation")
            .field("ordinal", &self.ordinal)
            .field("kind", &self.kind)
            .field("has_raw_eid", &self.raw_eid.is_some())
            .field("has_canonical_eid", &self.canonical_eid.is_some())
            .field("amount", &self.amount)
            .field("name", &"[REDACTED]")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// 详情页的一次原始观察；所有未判形状都有显式标记。
pub struct ConsignmentDetailObservation {
    /// 远端 MID。
    pub external_mail_id: String,
    /// 读取世界。
    pub world: MoogleMailWorld,
    /// 页面世界标记。
    pub world_marker: Option<MoogleMailWorld>,
    /// 列表页世界标记。
    pub list_world_marker: Option<MoogleMailWorld>,
    /// 完整收件人。
    pub recipient: String,
    /// 完整发件人。
    pub sender: String,
    /// 完整主题。
    pub subject: String,
    /// 原始正文；只在进程内用于证明，不进入展示或普通日志。
    pub body: String,
    /// 规范正文摘要。
    pub body_digest: [u8; 32],
    /// 主题摘要。
    pub subject_digest: [u8; 32],
    /// 原始收件分钟文本。
    pub received_minute_raw: String,
    /// 原始收件分钟解释后的 UTC 时刻。
    pub received_at: Timestamp,
    /// 页面声明的原始附件数；数量冲突时仍保留。
    pub reported_attachment_count: Option<u32>,
    /// Credits 金额；读不出时为空。
    pub credits: Option<u64>,
    /// COD 金额；读不出时为空。
    pub cod: Option<u64>,
    /// 服务器允许原样 Return。
    pub returnable: bool,
    /// 服务器允许该封 Take。
    pub claimable: bool,
    /// 动作按钮已证明。
    pub actions_proven: bool,
    /// 列表与详情身份一致。
    pub list_detail_consistent: bool,
    /// 重复原文 EID。
    pub duplicate_raw_eid: bool,
    /// 不同原文 EID 规范化为同一件。
    pub canonical_alias_ambiguous: bool,
    /// 未知附件布局或不可识别附件。
    pub unknown_attachment: bool,
    /// 声明数量与逐项清单不一致或数量缺失。
    pub asset_count_unknown: bool,
    /// 页面 `mail_state`；只在 1/2 时保留。
    pub mail_state: Option<u8>,
    /// 原序附件观察。
    pub items: Vec<ConsignmentAttachmentObservation>,
    /// 解析器版本。
    pub parser_version: &'static str,
}

impl fmt::Debug for ConsignmentDetailObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsignmentDetailObservation")
            .field("external_mail_id", &self.external_mail_id)
            .field("world", &self.world)
            .field("world_marker", &self.world_marker)
            .field("list_world_marker", &self.list_world_marker)
            .field("recipient", &"[REDACTED]")
            .field("sender", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .field("body_digest", &self.body_digest)
            .field("subject_digest", &self.subject_digest)
            .field("received_minute_raw", &self.received_minute_raw)
            .field("received_at", &self.received_at)
            .field("reported_attachment_count", &self.reported_attachment_count)
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("returnable", &self.returnable)
            .field("claimable", &self.claimable)
            .field("actions_proven", &self.actions_proven)
            .field("list_detail_consistent", &self.list_detail_consistent)
            .field("duplicate_raw_eid", &self.duplicate_raw_eid)
            .field("canonical_alias_ambiguous", &self.canonical_alias_ambiguous)
            .field("unknown_attachment", &self.unknown_attachment)
            .field("asset_count_unknown", &self.asset_count_unknown)
            .field("mail_state", &self.mail_state)
            .field("item_count", &self.items.len())
            .field("parser_version", &self.parser_version)
            .finish()
    }
}

/// 冻结清单中的一件附件；只含证明需要的公开事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenMailAttachment {
    /// 原序号。
    pub ordinal: u32,
    /// 显示名。
    pub name: String,
    /// 规范 EID。
    pub canonical_eid: Option<String>,
}

/// 可在写前逐项比较的整封冻结清单。
#[derive(Clone, PartialEq, Eq)]
pub struct FrozenMailManifest {
    /// 世界。
    pub world: MoogleMailWorld,
    /// 远端 MID。
    pub external_mail_id: String,
    /// 完整发件人。
    pub sender: String,
    /// 完整收件人。
    pub recipient: String,
    /// 主题。
    pub subject: String,
    /// 规范正文摘要。
    pub body_digest: [u8; 32],
    /// Credits。
    pub credits: u64,
    /// COD。
    pub cod: u64,
    /// 原始分钟文本。
    pub received_minute_raw: String,
    /// 原始分钟时刻。
    pub received_at: Timestamp,
    /// 原序附件。
    pub attachments: Vec<FrozenMailAttachment>,
    /// 与观察一致的清单摘要。
    pub manifest_digest: [u8; 32],
}

impl fmt::Debug for FrozenMailManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrozenMailManifest")
            .field("world", &self.world)
            .field("external_mail_id", &self.external_mail_id)
            .field("sender", &"[REDACTED]")
            .field("recipient", &"[REDACTED]")
            .field("subject", &"[REDACTED]")
            .field("body_digest", &self.body_digest)
            .field("credits", &self.credits)
            .field("cod", &self.cod)
            .field("received_minute_raw", &self.received_minute_raw)
            .field("received_at", &self.received_at)
            .field("attachment_count", &self.attachments.len())
            .field("manifest_digest", &self.manifest_digest)
            .finish()
    }
}

/// Take 成功后从响应页得到的受控观察；任何一项不成立都不授予处分权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeReceiptObservation {
    /// 身份与正文前缀仍匹配。
    pub identity_body_matches: bool,
    /// 资产已空。
    pub assets_empty: bool,
    /// 资产不可再取。
    pub assets_unavailable: bool,
    /// 写后 Credits。
    pub credits: i64,
    /// 写后 COD。
    pub cod: i64,
    /// 回执方向与金额相符。
    pub receipt_direction_matches: bool,
    /// 写后页面重新计算的清单摘要；不是写前冻结值的副本。
    pub after_page_digest: [u8; 32],
    /// 系统逐项 marker。
    pub markers: Vec<RemovedMarker>,
}
