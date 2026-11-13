//! Public 站点文档（规则、关于）读取端口。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ports::auction::ApplicationFuture;

/// 站点文档的种类闭集。规则与关于共用同一套多语言发布机制，按（种类，语言）各自独立：
/// 版本、发布命令与读取互不影响。存储层的表名沿用 `rules_documents`，种类是其中一列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SiteDocumentKind {
    /// 竞拍规则。
    Rules,
    /// 关于本系统。
    About,
}

impl SiteDocumentKind {
    /// 数据库与审计使用的稳定标识。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::About => "about",
        }
    }

    /// 解析存储中的标识；未知值失败关闭。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rules" => Some(Self::Rules),
            "about" => Some(Self::About),
            _ => None,
        }
    }
}

/// 公共端站点文档的默认语言。
pub const PUBLIC_RULES_DEFAULT_LOCALE: &str = "en";

/// 当前浏览器端支持的规则文档语言集合。
pub const PUBLIC_RULES_LOCALES: &[&str] = &["en", "zh-CN", "zh-TW", "ja", "ko"];

/// Public 站点文档查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRulesRequest {
    pub kind: SiteDocumentKind,
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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

pub trait PublicRulesReader: Send + Sync {
    fn get_public_rules(
        &self,
        request: PublicRulesRequest,
    ) -> ApplicationFuture<'_, Option<PublicRulesDocument>>;
}
