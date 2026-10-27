//! Public 规则文档查询与 Admin 发布适配器的稳定出口；规则事务和读模型位于子模块。

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
    RequestHash, TransactionFuture, TransactionPolicy,
};
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

mod reads;
#[cfg(feature = "postgres")]
mod transaction;
