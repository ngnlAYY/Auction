//! Admin 订单 resolve 测试共用的夹具：命令 ID、服务、输入构造与账本读数。
//!
//! 自 `mod.rs` 析出，使 `postgres_audit` 目标能以符号链接复用而不带入本目录的测试模块。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, DepositBusinessRejection, OrderAdminService, OrderResolveAction,
    ResolveOrderInput,
};
use auction_domain::CommandId;
use auction_storage::DatabaseStore;
use sqlx::PgPool;
use uuid::Uuid;

pub(in crate::consignment) const ADMIN: &str = "fake-admin";

/// 稳定的 UUIDv4 命令 ID：n 区分不同命令。
pub(in crate::consignment) fn command(n: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x7e20_0000_0000_4000_8000_0000_0000_0000 + n))
}

pub(in crate::consignment) fn service(store: DatabaseStore) -> OrderAdminService {
    OrderAdminService::new(Arc::new(store))
}

pub(in crate::consignment) fn resolve_input(
    n: u128,
    contract_id: Uuid,
    version: u64,
    action: OrderResolveAction,
    evidence_id: Uuid,
) -> ResolveOrderInput {
    ResolveOrderInput {
        admin_actor: ADMIN.to_owned(),
        command_id: command(n),
        contract_id,
        expected_version: version,
        action,
        evidence_id,
        request_id: None,
    }
}

/// 失败里的确定业务拒绝；其他错误返回空。
pub(in crate::consignment) fn rejection(
    error: &ConsignmentError,
) -> Option<DepositBusinessRejection> {
    match error {
        ConsignmentError::Deposit(rejection) => Some(*rejection),
        _ => None,
    }
}

pub(in crate::consignment) async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.expect("读取计数")
}

/// 账本里 resolve / recheck 行的 (结果状态, 结果码)；没有则为空。
pub(in crate::consignment) async fn ledger_of(
    pool: &PgPool,
    command_id: CommandId,
) -> Option<(i16, String)> {
    sqlx::query_as(
        "SELECT result_status, result_code FROM public.consignment_commands
         WHERE command_id = $1 AND operation IN ('resolve', 'recheck')",
    )
    .bind(command_id.as_uuid())
    .fetch_optional(pool)
    .await
    .expect("读取账本")
}
