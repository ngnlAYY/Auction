//! 标记 owner：Serializable 命令、逐条已提交竞价事实的提交后处理与独立发送证明。
//! 出价事务只追加 `lot_watch_events`；标记、领先周期与提醒在提交后的独立事务应用，
//! 不从 GET 或代理快照轮询恢复标记。

use crate::database::TrackedTransaction as Transaction;
mod commands;
mod facts;
mod outbox;
mod persistence;
mod preference;
mod proof;

pub(crate) use outbox::{
    bid_event, insert_bid_event as record_bid_event, insert_proxy_event as record_proxy_event,
    process_committed_events, proxy_event,
};
pub(crate) use proof::{
    delivery_eligible, delivery_watch_expired, delivery_watch_gate, delivery_watch_renamed,
    lock_delivery_lot,
};

use auction_application::{CompetitionTransactionError, LotWatchError};
use sqlx::Postgres;
use uuid::Uuid;

use persistence::{audit, invalidate, sql_error};
