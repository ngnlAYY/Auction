//! Admin 业务世界启停的命令、端口与事务边界。

use std::future::Future;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use auction_domain::{CommandId, valid_text};

use super::{ApplicationFuture, CompetitionTransactionError, RequestHash, WorldSnapshot};

/// 业务世界启停在共享命令账本中的稳定命令种类。
pub const ADMIN_WORLD_SET_ENABLED_COMMAND_KIND: &str = "admin_world_set_enabled";

#[derive(Debug, Clone)]
pub struct AdminWorldSetEnabledCommand {
    pub command_id: CommandId,
    pub world_id: String,
    pub enabled: bool,
    pub expected_policy_revision: i64,
    pub actor_id: String,
    pub request_id: Option<String>,
}

impl AdminWorldSetEnabledCommand {
    /// 校验不依赖数据库状态的闭集字段。
    pub fn validate(&self) -> Result<(), AdminWorldError> {
        if self.command_id.is_nil()
            || !matches!(self.world_id.as_str(), "main" | "isekai")
            || self.expected_policy_revision <= 0
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
        {
            return Err(AdminWorldError::InvalidCommand);
        }
        Ok(())
    }

    /// 生成不受 HTTP 编码影响的稳定请求摘要。
    pub fn request_hash(&self) -> Result<RequestHash, serde_json::Error> {
        #[derive(Serialize)]
        struct Digest<'a> {
            world_id: &'a str,
            enabled: bool,
            expected_policy_revision: i64,
            actor_id: &'a str,
        }

        RequestHash::from_request(
            ADMIN_WORLD_SET_ENABLED_COMMAND_KIND,
            admin_world_aggregate_id(&self.world_id),
            &Digest {
                world_id: &self.world_id,
                enabled: self.enabled,
                expected_policy_revision: self.expected_policy_revision,
                actor_id: &self.actor_id,
            },
        )
    }
}

pub trait AdminWorldPort: Send + Sync {
    fn list_worlds(&self) -> ApplicationFuture<'_, Vec<WorldSnapshot>>;

    fn set_enabled(
        &self,
        command: AdminWorldSetEnabledCommand,
    ) -> ApplicationFuture<'_, WorldSnapshot>;
}

/// PostgreSQL 完整档在共享竞价事务中提供的世界写入能力。
pub trait AdminWorldTransaction: Send {
    /// 按统一的 world-first 锁序读取权威状态。
    fn load_world_for_update(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<Option<WorldSnapshot>, CompetitionTransactionError>> + Send;

    /// 检查未终态拍卖或仍可能被调度领取的活动轮次。
    fn has_world_deactivation_blocker(
        &mut self,
        world_id: &str,
    ) -> impl Future<Output = Result<bool, CompetitionTransactionError>> + Send;

    /// 以 `policy_revision` 作为 CAS fence 更新业务世界状态。
    fn update_world_enabled(
        &mut self,
        world_id: &str,
        enabled: bool,
        expected_policy_revision: i64,
    ) -> impl Future<Output = Result<WorldSnapshot, CompetitionTransactionError>> + Send;

    /// 在状态与命令账本所在事务中追加 Admin 审计事实。
    fn append_admin_world_record(
        &mut self,
        command: AdminWorldSetEnabledCommand,
        previous: WorldSnapshot,
        result: WorldSnapshot,
        changed: bool,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// Admin 业务世界命令错误闭集。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminWorldError {
    #[error("业务世界命令无效")]
    InvalidCommand,
    #[error("业务世界不存在")]
    NotFound,
    #[error("世界仍有未终态拍卖或活动调度轮次")]
    ActiveAuction,
    #[error("业务世界策略版本冲突：当前版本 {current_revision}")]
    PolicyRevisionConflict { current_revision: i64 },
    #[error("业务世界命令与既有请求冲突")]
    CommandConflict,
    #[error("业务世界命令重放结果损坏")]
    StoredResultCorrupt,
}

/// 生成 world-scoped 的共享命令账本聚合标识。
pub fn admin_world_aggregate_id(world_id: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("auction/admin-world/{world_id}").as_bytes())
}
