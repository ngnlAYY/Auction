//! MoogleMail gateway 的配置和安全读取结果模型。

use super::*;

/// MoogleMail 收件箱 gateway 的有界扫描配置。
#[derive(Debug, Clone)]
pub struct MoogleMailGatewayConfig {
    /// 单次逻辑拉取最多读取的页数。
    pub maximum_pages: usize,
    /// 发件人 allowlist；默认只解析 allowlist 内邮件的详情。
    pub sender_allowlist: Vec<String>,
    /// 明确关闭 allowlist 后才允许读取所有发件人详情。
    pub sender_allowlist_disabled: bool,
}

impl Default for MoogleMailGatewayConfig {
    fn default() -> Self {
        Self {
            maximum_pages: MAX_HARD_PAGES,
            sender_allowlist: Vec::new(),
            sender_allowlist_disabled: false,
        }
    }
}

/// 已完成摘要/详情交叉校验的一次有界邮件拉取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoogleMailPage {
    /// 按最旧到最新顺序返回，便于应用层顺序入库。
    pub messages: Vec<MoogleMailMessage>,
    /// 本批最新已观测摘要的 external ID；空批由调用方保留旧 cursor。
    pub next_cursor: Option<String>,
}
