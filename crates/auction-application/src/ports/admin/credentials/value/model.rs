//! Admin 凭据闭集模型、安全投影和分页值对象。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::limits::{
    MAX_ADMIN_CREDENTIAL_OFFSET, MAX_ADMIN_CREDENTIAL_PAGE_SIZE, valid_text, valid_time,
};
use crate::ports::core::valid_page_window;

/// 旧版 Admin 可管理的外部凭据类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialKind {
    MailCookie,
    ForumCookie,
}

impl AdminCredentialKind {
    /// 返回数据库、HTTP 和请求摘要使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MailCookie => "mail_cookie",
            Self::ForumCookie => "forum_cookie",
        }
    }

    /// 从外部闭集字符串解析凭据类型。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mail_cookie" => Some(Self::MailCookie),
            "forum_cookie" => Some(Self::ForumCookie),
            _ => None,
        }
    }

    /// 返回凭据在 Worker 侧的稳定命名空间前缀。
    pub const fn scope_prefix(self) -> &'static str {
        match self {
            Self::MailCookie => "mooglemail",
            Self::ForumCookie => "ehentai",
        }
    }
}

/// Admin 凭据可绑定的旧版 world 范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialWorld {
    Main,
    Isekai,
    Shared,
}

impl AdminCredentialWorld {
    /// 返回 HTTP、摘要和管理端展示使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
            Self::Shared => "shared",
        }
    }

    /// 从外部闭集字符串解析 world 范围。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(Self::Main),
            "isekai" => Some(Self::Isekai),
            "shared" => Some(Self::Shared),
            _ => None,
        }
    }

    /// 将 Admin world 映射为数据库和 Worker 使用的内部 scope。
    pub fn scope(self, kind: AdminCredentialKind) -> String {
        format!("{}:{}", kind.scope_prefix(), self.as_str())
    }
}

/// 凭据当前状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminCredentialStatus {
    Active,
    Disabled,
    Invalid,
}

impl AdminCredentialStatus {
    /// 返回数据库、HTTP 和审计使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Invalid => "invalid",
        }
    }

    /// 从数据库字符串解析闭集状态。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "disabled" => Some(Self::Disabled),
            "invalid" => Some(Self::Invalid),
            _ => None,
        }
    }

    /// Admin API 允许的人工状态变更只包含 active/disabled；invalid 由 Worker 失败标记。
    pub const fn is_admin_selectable(self) -> bool {
        matches!(self, Self::Active | Self::Disabled)
    }
}

/// 不含密文、明文和密钥指纹的 Admin 凭据安全投影。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminCredential {
    pub id: Uuid,
    pub kind: AdminCredentialKind,
    pub world: AdminCredentialWorld,
    pub label: String,
    /// 可选预期名称，不表示已远端核验；历史命令结果缺字段按 None 读取。
    #[serde(default)]
    pub player_name: Option<String>,
    pub status: AdminCredentialStatus,
    pub state_version: i64,
    pub created_at: DateTime<Utc>,
    pub rotated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl AdminCredential {
    /// 校验 storage 返回的投影，防止未知 kind、scope 或非法版本进入 HTTP。
    pub fn validate(&self) -> bool {
        !self.id.is_nil()
            && valid_text(&self.label, 256)
            && crate::ports::admin::valid_credential_player_name(
                self.kind,
                self.player_name.as_deref(),
            )
            && self.state_version > 0
            && valid_time(self.created_at)
            && self.rotated_at.is_none_or(valid_time)
            && valid_time(self.updated_at)
            && self.updated_at >= self.created_at
            && self.rotated_at.is_none_or(|value| value >= self.created_at)
    }
}

/// Admin 凭据列表请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialListRequest {
    pub kind: Option<AdminCredentialKind>,
    pub world: Option<AdminCredentialWorld>,
    pub status: Option<AdminCredentialStatus>,
    pub limit: i64,
    pub offset: i64,
}

impl AdminCredentialListRequest {
    /// 校验闭集筛选和有界数字分页。
    pub fn validate(&self) -> bool {
        valid_page_window(
            self.limit,
            self.offset,
            MAX_ADMIN_CREDENTIAL_PAGE_SIZE,
            MAX_ADMIN_CREDENTIAL_OFFSET,
        )
    }
}

/// Admin 凭据列表页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCredentialPage {
    pub credentials: Vec<AdminCredential>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl AdminCredentialPage {
    /// 校验页边界、筛选命中和稳定排序。
    pub fn validate(&self, request: &AdminCredentialListRequest) -> bool {
        request.validate()
            && self.total >= 0
            && self.limit == request.limit
            && self.offset == request.offset
            && i64::try_from(self.credentials.len())
                .is_ok_and(|credential_count| credential_count <= self.limit)
            && self.credentials.iter().enumerate().all(|(index, credential)| {
                credential.validate()
                    && request.kind.is_none_or(|kind| credential.kind == kind)
                    && request.world.is_none_or(|world| credential.world == world)
                    && request.status.is_none_or(|status| credential.status == status)
                    && self.credentials.get(index + 1).is_none_or(|next| {
                        credential.created_at > next.created_at
                            || credential.created_at == next.created_at && credential.id > next.id
                    })
            })
    }
}
