//! Public 黑名单读取端口。

use chrono::{DateTime, Utc};

use crate::ApplicationFuture;

/// Public 黑名单允许的最大页大小。
pub const MAX_PUBLIC_BLACKLIST_PAGE_SIZE: i64 = 200;

/// Public 黑名单编号页允许的最大偏移。
pub const MAX_PUBLIC_BLACKLIST_OFFSET: i64 = 2_147_483_647;

/// Public 黑名单分页请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicBlacklistRequest {
    pub limit: i64,
    pub offset: i64,
}

impl PublicBlacklistRequest {
    /// 校验编号页边界，保持旧接口的有限查询成本。
    pub fn validate(self) -> Result<(), &'static str> {
        if self.limit < 1
            || self.limit > MAX_PUBLIC_BLACKLIST_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_BLACKLIST_OFFSET
        {
            return Err("公开黑名单分页参数无效");
        }
        Ok(())
    }
}

/// Public 黑名单条目的安全投影，不含内部用户 ID 或运营 actor。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicBlacklistEntry {
    pub game_player_id: String,
    pub public_reason: String,
    pub blacklisted_at: DateTime<Utc>,
}

/// Public 黑名单分页结果。
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
        if self.total < 0
            || self.limit < 1
            || self.limit > MAX_PUBLIC_BLACKLIST_PAGE_SIZE
            || self.offset < 0
            || self.offset > MAX_PUBLIC_BLACKLIST_OFFSET
            || self.entries.len() as i64 > self.limit
            || self.total < self.entries.len() as i64
            || self.entries.iter().any(|entry| {
                entry.game_player_id.trim() != entry.game_player_id
                    || entry.game_player_id.is_empty()
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

/// Public 黑名单读取端口。
pub trait PublicBlacklistReader: Send + Sync {
    fn list_public_blacklist(
        &self,
        request: PublicBlacklistRequest,
    ) -> ApplicationFuture<'_, PublicBlacklistPage>;
}
