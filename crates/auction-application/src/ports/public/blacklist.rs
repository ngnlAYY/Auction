//! Public 黑名单读取端口。

use chrono::{DateTime, Utc};

use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::auction::ApplicationFuture;

/// Public 黑名单编号页允许的最大偏移。
pub const MAX_PUBLIC_BLACKLIST_OFFSET: i64 = 2_147_483_647;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicBlacklistRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PublicBlacklistRequest {
    /// 数量仅要求正整数；保留编号页的既有 offset 防护。
    pub fn validate(self) -> Result<(), &'static str> {
        if self.limit < 1 || self.offset < 0 || self.offset > MAX_PUBLIC_BLACKLIST_OFFSET {
            return Err("公开黑名单分页参数无效");
        }
        Ok(())
    }
}

/// Public 黑名单条目的安全投影，不含内部用户 ID 或运营 actor。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBlacklistEntry {
    pub username: String,
    pub public_reason: String,
    pub blacklisted_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBlacklistPage {
    pub entries: Vec<PublicBlacklistEntry>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl PublicBlacklistPage {
    /// 校验 owner 返回的页边界和公开字段。
    pub fn validate(&self) -> Result<(), &'static str> {
        let entry_count = i64::try_from(self.entries.len()).map_err(|error| {
            record_error(
                DiagnosticLevel::Error,
                error.diagnostic_kind(),
                "public_blacklist",
                "convert_entry_count",
                DiagnosticContext::default(),
            );
            "公开黑名单页大小溢出"
        })?;
        if self.total < 0
            || self.limit < 1
            || self.offset < 0
            || self.offset > MAX_PUBLIC_BLACKLIST_OFFSET
            || entry_count > self.limit
            || self.total < entry_count
            || self.entries.iter().any(|entry| {
                entry.username.trim() != entry.username
                    || entry.username.is_empty()
                    || entry.public_reason.trim() != entry.public_reason
                    || entry.public_reason.is_empty()
                    || entry.blacklisted_at.timestamp_micros() == 0
            })
        {
            return Err("公开黑名单页结果无效");
        }
        Ok(())
    }
}

pub trait PublicBlacklistReader: Send + Sync {
    fn list_public_blacklist(
        &self,
        request: PublicBlacklistRequest,
    ) -> ApplicationFuture<'_, PublicBlacklistPage>;
}
