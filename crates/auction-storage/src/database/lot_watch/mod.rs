//! 标记 owner：Serializable 命令、逐条竞价事实与独立发送证明。
//! 所有业务写入持有竞价共用 lot 锁；不从 GET 或代理快照轮询恢复标记。

use crate::database::TrackedTransaction as Transaction;
mod commands;
mod facts;
mod persistence;
mod preference;
mod proof;

pub(crate) use facts::{apply_bid, auto_watch_bid, auto_watch_proxy};
pub(crate) use proof::{
    delivery_eligible, delivery_watch_expired, delivery_watch_gate, lock_delivery_lot,
};

use auction_application::{CompetitionTransactionError, LotWatchError};
use sqlx::Postgres;
use uuid::Uuid;

use persistence::{audit, invalidate, sql_error};
