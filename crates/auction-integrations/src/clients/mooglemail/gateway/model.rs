//! MoogleMail gateway 的配置和安全读取结果模型。

use super::*;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

/// MoogleMail 收件箱 gateway 的有界扫描配置。
#[derive(Debug, Clone)]
pub struct MoogleMailGatewayConfig {
    pub maximum_pages: usize,
    /// 发件人 allowlist；默认只解析 allowlist 内邮件的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
    /// 单次 inbox 扫描及详情读取共享的总预算，覆盖限速、凭据切换和 HTTP 正文读取。
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
#[derive(Debug, Clone)]
pub struct MoogleMailSentLookupRequest {
    pub world: MoogleMailWorld,
    pub recipient: String,
    pub subject: String,
    pub external_equipment_id: String,
    pub maximum_pages: usize,
    pub deadline: Instant,
    pub observed_at: DateTime<Utc>,
}

impl MoogleMailSentLookupRequest {
    pub fn validate(&self, configured_pages: usize) -> bool {
        self.recipient.len() <= 256
            && self.recipient == self.recipient.trim()
            && !self.recipient.is_empty()
            && !self.recipient.chars().any(char::is_control)
            && self.subject.len() <= 512
            && self.subject == self.subject.trim()
            && !self.subject.is_empty()
            && !self.subject.chars().any(char::is_control)
            && self.external_equipment_id.len() <= 32
            && self.external_equipment_id.bytes().all(|byte| byte.is_ascii_digit())
            && (self.external_equipment_id.len() == 1
                || self.external_equipment_id.as_bytes()[0] != b'0')
            && !self.external_equipment_id.is_empty()
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
    pub observed_at: DateTime<Utc>,
}

/// 已完成摘要/详情交叉校验的一次有界邮件拉取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailPage {
    /// 按最旧到最新顺序返回，便于应用层顺序入库。
    pub messages: Vec<MoogleMailMessage>,
    /// 本批最新已观测摘要的 external ID；空批由调用方保留旧 cursor。
    pub next_cursor: Option<String>,
}
