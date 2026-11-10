//! Public 规则文档查询与 Admin 发布适配器的稳定出口；规则事务和读模型位于子模块。

use crate::database::DatabaseStore;

mod reads;
mod transaction;
