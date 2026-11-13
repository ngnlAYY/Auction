//! 账号级竞价警告偏好；独立版本，不改变已有标记及已入队通知。

use crate::ports::public::{LotWatchError, LotWatchFuture, LotWatchTransaction};
use auction_domain::CommandId;
use serde::{Deserialize, Serialize};
use std::future::Future;
use uuid::Uuid;

/// 权威账号偏好，默认 mm/1。`channel` 是闭集 `none`/`mm`/`pm`/`both`：MM 与 PM 各自独立，
/// 可同时启用，也可都关闭（标记只作收藏，不产生提醒）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LotWatchPreference {
    pub user_id: Uuid,
    pub channel: String,
    pub version: i64,
}

/// 稳定完整命令；同渠道也推进 fence。
#[derive(Debug, Clone, Serialize)]
pub struct SetLotWatchPreferenceCommand {
    pub command_id: CommandId,
    pub user_id: Uuid,
    pub channel: String,
    pub expected_version: i64,
}

/// 当前会话账号的服务器权威偏好。
pub trait LotWatchPreferencePort: Send + Sync {
    fn get(&self, user_id: Uuid) -> LotWatchFuture<'_, LotWatchPreference>;
    fn set(&self, command: SetLotWatchPreferenceCommand) -> LotWatchFuture<'_, LotWatchPreference>;
    /// PM 渠道可用性 = 论坛同步已启用且账号已绑定论坛账号。
    fn pm_available(&self, user_id: Uuid) -> LotWatchFuture<'_, bool>;
}

/// 复用账本，偏好只锁 user；不得倒取 lot/watch 锁。
pub trait LotWatchPreferenceTransaction: LotWatchTransaction {
    fn lock_preference(
        &mut self,
        user_id: Uuid,
    ) -> impl Future<Output = Result<LotWatchPreference, LotWatchError>> + Send;
    fn write_preference(
        &mut self,
        command: &SetLotWatchPreferenceCommand,
        next: i64,
    ) -> impl Future<Output = Result<(), LotWatchError>> + Send;
    /// 事务内复核 PM 渠道可用性；不可用时拒绝保存并说明原因。
    fn forum_pm_available(
        &mut self,
        user_id: Uuid,
    ) -> impl Future<Output = Result<bool, LotWatchError>> + Send;
}
