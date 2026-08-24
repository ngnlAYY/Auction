//! Public 规则文档查询与 Admin 发布适配器的稳定出口。
//!
//! PostgreSQL 规则事务和跨后端读模型分别归档到子模块；本模块只保留共享依赖、模块装配
//! 与 `DatabaseStore` 的存储端口边界。

#[cfg(feature = "postgres")]
use std::future::Future;

use auction_application::{
    AdminRulesDocument, AdminRulesReader, AdminRulesRequest, ApplicationError, ApplicationFuture,
    PublicRulesDocument, PublicRulesReader, PublicRulesRequest,
};
#[cfg(feature = "postgres")]
use auction_application::{
    AdminRulesPublishCommand, AdminRulesStoredCommand, AdminRulesTransaction,
    AdminRulesTransactionError, AdminRulesTransactionFailure, AdminRulesTransactionRunner,
    RequestHash, TransactionFuture, TransactionIsolation, TransactionPolicy,
};
#[cfg(feature = "sqlite")]
use chrono::NaiveDateTime;
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::{Value, json};
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::{Postgres, Transaction};
#[cfg(feature = "postgres")]
use uuid::Uuid;

#[cfg(feature = "postgres")]
use crate::database::audit_health::AuditWriteResult as _;

use super::DatabaseStore;

#[cfg(feature = "postgres")]
use super::PostgresStore;
#[cfg(feature = "sqlite")]
use super::SqliteStore;

mod reads;
#[cfg(feature = "postgres")]
mod transaction;
