//! Public account 查询结果的映射模块装配。
//!
//! 普通/代理竞价事实、代理状态投影以及跨后端读取与分页校验分别由子模块拥有；本模块只
//! 负责稳定导出，数据库查询仍由同级 PostgreSQL/SQLite owner 持有。

use super::*;

#[path = "bids.rs"]
mod bids;
#[path = "common.rs"]
mod common;
#[path = "proxy.rs"]
mod proxy;

pub(super) use bids::*;
pub(super) use common::*;
pub(super) use proxy::*;
