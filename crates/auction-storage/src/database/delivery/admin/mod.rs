//! Admin 履约查询与命令适配；复用应用状态机、持久命令账本和统一错误分类。

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError, DeliveryAdminListRequest,
    DeliveryAdminPage, DeliveryAdminPort,
};
use uuid::Uuid;

use crate::database::DatabaseStore;

/// 只在扩展投影（`include=settlement_returns`）中出现的投递（参数为 `public.deliveries` 的别名）：
/// 结算退件 Take（来源 `settlement_return_action`），以及恢复接管退件后由恢复 owner 在收口时终结的
/// 原结算投递（`failed/recovery_superseded`，从未进入发送围栏、没有发送时间，已发布页面按旧时间链
/// 校验会整页解码失败）。列表、计数、详情与玩家支持在同一条件下排除，分页与总数一致。
macro_rules! extended_projection_delivery_sql {
    ($delivery:literal) => {
        concat!(
            "(",
            $delivery,
            ".origin_kind = 'settlement_return_action' OR (",
            $delivery,
            ".status = 'failed' AND ",
            $delivery,
            ".failure_code = 'recovery_superseded'))"
        )
    };
}

mod commands;
mod detail;
mod eligibility;
mod mapping;
mod reads;
mod support;
mod transaction;

#[cfg(feature = "test-db")]
pub use mapping::send_category;

impl DeliveryAdminPort for DatabaseStore {
    fn list(
        &self,
        request: DeliveryAdminListRequest,
    ) -> auction_application::core::ApplicationFuture<'_, DeliveryAdminPage> {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            reads::list(self.pool(), &request).await.map_err(ApplicationError::from)
        })
    }

    fn detail(
        &self,
        delivery_id: Uuid,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        auction_application::delivery::DeliveryAdminDetail,
    > {
        Box::pin(async move {
            if delivery_id.is_nil() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            detail::detail(self.pool(), delivery_id).await.map_err(ApplicationError::from)
        })
    }

    fn detail_with_guard(
        &self,
        delivery_id: Uuid,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        (
            auction_application::delivery::DeliveryAdminDetail,
            auction_application::delivery::DeliveryAdminGuardSnapshot,
            Option<auction_application::delivery::DeliveryAdminGuardBlocker>,
        ),
    > {
        Box::pin(async move {
            if delivery_id.is_nil() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            detail::detail_with_guard(self.pool(), delivery_id)
                .await
                .map_err(ApplicationError::from)
        })
    }

    fn player_support(
        &self,
        request: auction_application::delivery::PlayerSupportRequest,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        auction_application::delivery::PlayerSupportBundle,
    > {
        Box::pin(async move {
            if !request.validate() {
                return Err(DeliveryAdminError::Invalid.into());
            }
            support::player_support(self.pool(), &request).await.map_err(ApplicationError::from)
        })
    }

    fn execute(
        &self,
        command: DeliveryAdminCommand,
    ) -> auction_application::core::ApplicationFuture<
        '_,
        auction_application::delivery::DeliveryAdminResult,
    > {
        Box::pin(async move {
            validate_command(&command)?;
            commands::execute(self.pool(), &command).await.map_err(ApplicationError::from)
        })
    }
}

fn validate_command(command: &DeliveryAdminCommand) -> Result<(), DeliveryAdminError> {
    if command.command_id.is_nil()
        || command.delivery_id.is_nil()
        || command.expected_fencing_version <= 0
        || command.actor_id.is_empty()
        || command.actor_id != command.actor_id.trim()
        || command
            .request_id
            .as_ref()
            .is_some_and(|value| value.is_empty() || value != value.trim())
        || command.request_hash.as_bytes() == [0; 32]
    {
        return Err(DeliveryAdminError::Invalid);
    }
    match command.action {
        DeliveryAdminAction::Release
        | DeliveryAdminAction::RetryFailed
        | DeliveryAdminAction::ReconcileRetry
            if command.external_reference.is_none() && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileSucceeded
            if command
                .external_reference
                .as_ref()
                .is_some_and(|value| !value.is_empty() && value == value.trim())
                && command.failure_code.is_none() => {}
        DeliveryAdminAction::ReconcileFailed
            if command.external_reference.is_none()
                && command
                    .failure_code
                    .as_ref()
                    .is_some_and(|value| !value.is_empty() && value == value.trim()) => {}
        _ => return Err(DeliveryAdminError::Invalid),
    }
    Ok(())
}

/// Admin 投递读取与命令的 SQL 失败对外统一为 `Unavailable`；丢弃前记录安全投影，
/// 授权缺失、语句超时或列漂移等故障才能按 SQLSTATE 定位。
pub(crate) fn map_storage_error(error: sqlx::Error) -> DeliveryAdminError {
    crate::database::log_discarded_sql_error(
        &error,
        "delivery_admin_storage_failed",
        "delivery Admin 数据库操作失败",
    );
    DeliveryAdminError::Unavailable
}
