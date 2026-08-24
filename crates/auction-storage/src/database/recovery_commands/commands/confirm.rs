//! Recovery 确认、调度推进与命令审计的模块装配。
//!
//! 三类 PostgreSQL 写入路径保持各自职责；本文件只保留父模块使用的稳定内部出口。

#[cfg(feature = "postgres")]
use super::*;

#[cfg(feature = "postgres")]
#[path = "confirm/command.rs"]
mod command;
#[cfg(feature = "postgres")]
#[path = "confirm/common.rs"]
mod common;
#[cfg(feature = "postgres")]
#[path = "confirm/refund.rs"]
mod refund;
#[cfg(feature = "postgres")]
#[path = "confirm/relist.rs"]
mod relist;

#[cfg(feature = "postgres")]
pub(super) use command::*;
#[cfg(feature = "postgres")]
pub(super) use common::*;
#[cfg(feature = "postgres")]
pub(super) use refund::*;
#[cfg(feature = "postgres")]
pub(super) use relist::*;
