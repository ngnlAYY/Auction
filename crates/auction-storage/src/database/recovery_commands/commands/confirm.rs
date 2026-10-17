//! Recovery 确认、调度推进与命令审计的模块装配；三类写入路径各自独立，这里只保留稳定出口。

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
