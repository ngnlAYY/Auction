//! Admin Mail owner 的查询、命令和事务窄端口。

use std::future::Future;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use auction_domain::CommandId;

use super::commands::{
    AdminMailPollTriggerCommand, AdminMailPollingCommand, AdminMailStoredCommand,
};
use super::errors::{AdminMailTransactionError, AdminMailTransactionFailure};
use super::model::{
    AdminMailDashboardSnapshot, AdminMailIntakeDetail, AdminMailIntakeFilter, AdminMailIntakePage,
    AdminMailPollState, AdminMailPollStateRequest,
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::{TransactionFuture, TransactionPolicy};

/// Admin 邮件只读端口。
pub trait AdminMailReader: Send + Sync {
    /// 读取所有邮箱轮询器的安全 dashboard 摘要，不返回同步游标。
    fn dashboard_snapshot(&self) -> ApplicationFuture<'_, AdminMailDashboardSnapshot>;

    fn poll_state(
        &self,
        request: AdminMailPollStateRequest,
    ) -> ApplicationFuture<'_, AdminMailPollState>;

    fn list_intakes(
        &self,
        request: AdminMailIntakeFilter,
    ) -> ApplicationFuture<'_, AdminMailIntakePage>;

    fn intake_detail(
        &self,
        intake_id: Uuid,
    ) -> ApplicationFuture<'_, Option<AdminMailIntakeDetail>>;
}

pub trait AdminMailCommandPort: Send + Sync {
    fn set_polling_paused(
        &self,
        command: AdminMailPollingCommand,
    ) -> ApplicationFuture<'_, AdminMailPollState>;

    /// 将指定邮箱的 next_poll_at 安全提前到当前时间，并通过命令账本保证幂等。
    fn trigger_poll(
        &self,
        command: AdminMailPollTriggerCommand,
    ) -> ApplicationFuture<'_, AdminMailPollState>;
}

/// Admin 邮件完整端口；装配根可以用一个 trait object 注入同一 owner。
pub trait AdminMailPort: AdminMailReader + AdminMailCommandPort {}

impl<T> AdminMailPort for T where T: AdminMailReader + AdminMailCommandPort {}

/// 邮件暂停命令事务的窄适配器能力。
pub trait AdminMailTransaction: Send {
    fn find_command(
        &mut self,
        command_id: CommandId,
    ) -> impl Future<Output = Result<Option<AdminMailStoredCommand>, AdminMailTransactionError>> + Send;

    fn insert_command(
        &mut self,
        command: AdminMailStoredCommand,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send;

    fn set_polling_paused(
        &mut self,
        command: AdminMailPollingCommand,
        updated_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminMailPollState, AdminMailTransactionError>> + Send;

    fn append_pause_record(
        &mut self,
        command: AdminMailPollingCommand,
        result: AdminMailPollState,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send;

    fn trigger_poll(
        &mut self,
        command: AdminMailPollTriggerCommand,
        triggered_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<AdminMailPollState, AdminMailTransactionError>> + Send;

    fn append_trigger_record(
        &mut self,
        command: AdminMailPollTriggerCommand,
        result: AdminMailPollState,
    ) -> impl Future<Output = Result<(), AdminMailTransactionError>> + Send;
}

/// 邮件暂停命令事务 runner；PostgreSQL 使用 Serializable 与有限重试。
pub trait AdminMailTransactionRunner: Send + Sync {
    type Transaction: AdminMailTransaction;

    fn run<T, E, F>(
        &self,
        policy: TransactionPolicy,
        operation: F,
    ) -> impl Future<Output = Result<T, E>> + Send
    where
        T: Send,
        E: AdminMailTransactionFailure,
        F: for<'a> Fn(&'a mut Self::Transaction) -> TransactionFuture<'a, T, E> + Send;
}
