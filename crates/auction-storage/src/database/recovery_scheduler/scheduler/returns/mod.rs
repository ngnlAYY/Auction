//! Recovery 退回履约记录与下一步 action 建立；子模块承载编排、证据、校验、幂等写入和行映射，
//! record_return_postgres 仍是唯一事务入口，外部副作用由后续 delivery owner 负责。

mod action;
mod mapping;
mod model;
mod record;
mod validation;

#[cfg(feature = "postgres")]
pub(super) use record::record_return_postgres;
