//! Public 世界目录读取端口。

use crate::ApplicationFuture;

/// 世界目录的应用层投影，不暴露数据库记录或 SQL 类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldSnapshot {
    pub id: String,
    pub enabled: bool,
    pub availability: String,
    pub policy_revision: i64,
}

/// Public 世界目录读取端口。
pub trait WorldReader: Send + Sync {
    /// 返回当前可公开展示的世界，排序和可见性由持久化 owner 保证。
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>>;
}
