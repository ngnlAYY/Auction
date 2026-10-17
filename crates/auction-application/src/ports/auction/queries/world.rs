//! Public 世界目录读取端口。

use serde::{Deserialize, Serialize};

use crate::ApplicationFuture;

/// 世界目录的应用层投影，不暴露数据库记录或 SQL 类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSnapshot {
    pub id: String,
    pub enabled: bool,
    pub availability: String,
    pub policy_revision: i64,
}

impl WorldSnapshot {
    /// 校验数据库与命令账本返回的业务世界安全投影。
    pub fn validate(&self) -> bool {
        matches!(self.id.as_str(), "main" | "isekai")
            && matches!(self.availability.as_str(), "available" | "degraded" | "unavailable")
            && self.policy_revision > 0
    }
}

pub trait WorldReader: Send + Sync {
    /// 返回当前可公开展示的世界，排序和可见性由持久化 owner 保证。
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>>;
}
