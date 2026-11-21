//! 寄售事务的 SQLSTATE 分类与脱敏错误边界。

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_domain::consignment::ListingError;
use thiserror::Error;

use crate::database::backend::{
    is_serialization_conflict, observe_transaction_error, sql_error_sqlstate,
};

/// 寄售事务内部错误。不变量 6、7：仅保留安全诊断字段及明确的重试判定。
#[derive(Debug, Error)]
pub(crate) enum WriteError {
    /// PostgreSQL 明确中止，可以重开整个事务。
    #[error("寄售事务发生并发冲突")]
    RetryConflict,
    /// 可转换为应用端结果的安全错误。
    #[error(transparent)]
    Application(#[from] ConsignmentError),
    /// 被显式命名的当前比较钥唯一索引拒绝。调用方通过单例行锁或插入仲裁避免触发。
    #[error("寄售当前比较钥唯一约束冲突")]
    ReplayKeyCurrent,
    /// 未知约束或持久事实失配。
    #[error("寄售存储异常 [sqlstate={sqlstate} constraint={constraint}]")]
    Invariant { sqlstate: String, constraint: String },
}

impl WriteError {
    /// 适配器的内部校验失败。原因只允许固定机器码。
    pub(in crate::database::consignment) fn invariant(reason: &'static str) -> Self {
        Self::Invariant { sqlstate: "none".into(), constraint: reason.into() }
    }

    /// 将脱敏的内部错误投影为应用端口错误。
    #[allow(dead_code, reason = "由后续任务 2D / 2F 调用")]
    pub(crate) fn into_application(self) -> ConsignmentError {
        match self {
            Self::Application(error) => error,
            Self::RetryConflict | Self::ReplayKeyCurrent | Self::Invariant { .. } => {
                tracing::warn!(error = %self, "寄售存储事务失败");
                ConsignmentError::Unavailable
            }
        }
    }
}

/// 已持久列的领域值损坏时拒绝继续读取或写入。
pub(in crate::database::consignment) fn invalid_projection(error: ListingError) -> WriteError {
    WriteError::invariant(match error {
        ListingError::InvalidIdentity => "consignment_listing_identity",
        ListingError::InvalidPrice => "consignment_listing_price",
        ListingError::InvalidFee => "consignment_listing_fee",
        _ => "consignment_listing_projection",
    })
}

impl From<sqlx::Error> for WriteError {
    fn from(error: sqlx::Error) -> Self {
        observe_transaction_error(&error);
        if is_serialization_conflict(&error) {
            return Self::RetryConflict;
        }
        let sqlstate = sql_error_sqlstate(&error);
        let constraint = error.as_database_error().and_then(sqlx::error::DatabaseError::constraint);
        // 仅按 SQLSTATE 和显式约束名分类。不变量 6、7：不读取 MESSAGE、DETAIL 或错误链。
        let business = match (sqlstate.as_ref(), constraint) {
            ("23505", Some("consignment_listings_seller_active_idx")) => {
                ListingBusinessRejection::AlreadyOpen
            }
            ("23505", Some("consignment_listings_equipment_published_idx")) => {
                ListingBusinessRejection::EquipmentBusy
            }
            ("23505", Some("consignment_replay_keys_write_current_idx")) => {
                return Self::ReplayKeyCurrent;
            }
            ("55P03" | "57014" | "25P04", _) => return ConsignmentError::Unavailable.into(),
            _ => {
                let constraint = constraint.filter(|name| {
                    !name.is_empty()
                        && name.len() <= 63
                        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                });
                return Self::Invariant {
                    sqlstate: sqlstate.into_owned(),
                    constraint: constraint.unwrap_or("none").into(),
                };
            }
        };
        ConsignmentError::from(business).into()
    }
}

/// 提交阶段的连接或协议中断一律作为未知结果，不变量 1、4。
pub(super) fn commit_error(error: sqlx::Error) -> WriteError {
    let sqlstate = sql_error_sqlstate(&error);
    if error.as_database_error().is_none()
        || sqlstate.starts_with("08")
        || matches!(sqlstate.as_ref(), "40003" | "57P01" | "57P02" | "57P05" | "invalid" | "none")
    {
        observe_transaction_error(&error);
        return ConsignmentError::OutcomeUnknown.into();
    }
    error.into()
}
