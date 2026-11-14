//! Admin lot 修改命令端口。
//! 该模块只声明 draft lot 的窄修改集合和事务能力。具体锁顺序、SQL、预览修订、审计、实时
//! 事件及 withdrawn-return 义务由完整档存储适配器实现；HTTP 不得直接修改 lot 字段。

use std::future::Future;

use auction_domain::{AuctionId, CommandId, LotId, valid_text};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ports::admin::AdminLotSummary;
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{CompetitionTransactionError, RequestHash, StoredCommand};

/// draft lot 允许的修改动作闭集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminLotAction {
    Withdraw,
    SetStartPrice,
}

impl AdminLotAction {
    /// 返回命令账本和审计使用的稳定动作名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Withdraw => "withdraw",
            Self::SetStartPrice => "set_start_price",
        }
    }

    /// 返回命令账本中的稳定命令种类。
    pub const fn command_kind(self) -> &'static str {
        match self {
            Self::Withdraw => "admin_lot_withdraw",
            Self::SetStartPrice => "admin_lot_set_start_price",
        }
    }
}

/// Admin lot 修改命令；版本 fence、幂等摘要和审计身份必须由应用层携带。
#[derive(Debug, Clone)]
pub struct AdminLotUpdateCommand {
    pub command_id: CommandId,
    pub lot_id: LotId,
    pub expected_state_version: i64,
    pub action: AdminLotAction,
    pub start_price: Option<i64>,
    pub actor_id: String,
    pub request_id: Option<String>,
    pub request_hash: RequestHash,
}

impl AdminLotUpdateCommand {
    /// 校验 action 专属字段、版本 fence 和审计身份。
    pub fn validate(&self) -> Result<(), AdminLotUpdateError> {
        let action_fields_valid = match self.action {
            AdminLotAction::Withdraw => self.start_price.is_none(),
            AdminLotAction::SetStartPrice => self.start_price.is_some_and(|value| value > 0),
        };
        if self.command_id.is_nil()
            || self.lot_id.is_nil()
            || self.expected_state_version <= 0
            || !action_fields_valid
            || !valid_text(&self.actor_id, 128)
            || self.request_id.as_deref().is_some_and(|value| !valid_text(value, 128))
            || self.request_hash.as_bytes() == [0; 32]
        {
            return Err(AdminLotUpdateError::InvalidCommand);
        }
        Ok(())
    }
}

/// Admin lot 修改前锁内读取的安全快照。
#[derive(Debug, Clone)]
pub struct AdminLotUpdateSnapshot {
    pub lot: AdminLotSummary,
    pub auction_status: String,
    pub world_id: String,
    pub auction_state_version: i64,
    pub preview_revision: i64,
    pub policy_version: i32,
}

impl AdminLotUpdateSnapshot {
    /// 校验锁内跨表投影，避免把错误归属或无效策略版本带入履约义务。
    pub fn validate(&self) -> bool {
        self.lot.validate()
            && matches!(
                self.auction_status.as_str(),
                "draft" | "preview" | "live" | "final" | "settling" | "settled" | "cancelled"
            )
            && valid_text(&self.world_id, 64)
            && self.auction_state_version > 0
            && self.preview_revision >= 0
            && self.policy_version >= 0
    }
}

pub trait AdminLotUpdatePort: Send + Sync {
    fn update(&self, command: AdminLotUpdateCommand) -> ApplicationFuture<'_, AdminLotSummary>;
}

/// Admin lot 修改使用的 PostgreSQL 完整档事务能力。
pub trait AdminLotUpdateTransaction: Send {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<StoredCommand>, CompetitionTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: StoredCommand,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    fn load_admin_lot_for_update(
        &mut self,
        lot_id: LotId,
    ) -> impl Future<Output = Result<Option<AdminLotUpdateSnapshot>, CompetitionTransactionError>> + Send;

    fn update_admin_lot(
        &mut self,
        command: &AdminLotUpdateCommand,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminLotSummary, CompetitionTransactionError>> + Send;

    fn advance_admin_preview_revision(
        &mut self,
        auction_id: AuctionId,
        expected_revision: i64,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<i64, CompetitionTransactionError>> + Send;

    fn insert_withdrawn_return_obligation(
        &mut self,
        command: &AdminLotUpdateCommand,
        snapshot: &AdminLotUpdateSnapshot,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;

    fn append_admin_lot_record(
        &mut self,
        command: &AdminLotUpdateCommand,
        result: &AdminLotSummary,
        changed: bool,
        auction_state_version: i64,
        preview_revision: i64,
    ) -> impl Future<Output = Result<(), CompetitionTransactionError>> + Send;
}

/// Admin lot 命令的输入错误。
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AdminLotUpdateError {
    #[error("后台 lot 修改命令无效")]
    InvalidCommand,
}
