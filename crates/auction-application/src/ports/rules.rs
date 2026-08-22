//! Public 规则文档读取端口。

use chrono::{DateTime, Utc};

use crate::ApplicationFuture;

/// 公共端规则文档的默认语言。
pub const PUBLIC_RULES_DEFAULT_LOCALE: &str = "en";

/// 当前浏览器端支持的规则文档语言集合。
pub const PUBLIC_RULES_LOCALES: &[&str] = &["en", "zh-CN", "zh-TW", "ja", "ko"];

/// Public 规则查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRulesRequest {
    pub locale: String,
}

impl PublicRulesRequest {
    /// 只允许已冻结语言，避免把任意路径或控制字符传给规则 owner。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.locale.trim() != self.locale
            || self.locale.is_empty()
            || self.locale.len() > 16
            || self.locale.chars().any(char::is_control)
            || !PUBLIC_RULES_LOCALES.contains(&self.locale.as_str())
        {
            return Err("规则语言参数无效");
        }
        Ok(())
    }
}

/// 已通过应用边界校验的公开规则文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRulesDocument {
    pub locale: String,
    pub title: String,
    pub markdown: String,
    pub revision: i64,
    pub published_at: DateTime<Utc>,
}

impl PublicRulesDocument {
    /// 拒绝不完整快照，避免 HTTP 层返回不可用的规则内容。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.locale.trim() != self.locale
            || !PUBLIC_RULES_LOCALES.contains(&self.locale.as_str())
            || self.title.trim() != self.title
            || self.title.is_empty()
            || self.revision <= 0
            || self.published_at.timestamp_micros() == 0
        {
            return Err("规则文档快照无效");
        }
        Ok(())
    }
}

/// Public 规则文档读取端口。
pub trait PublicRulesReader: Send + Sync {
    fn get_public_rules(
        &self,
        request: PublicRulesRequest,
    ) -> ApplicationFuture<'_, Option<PublicRulesDocument>>;
}
