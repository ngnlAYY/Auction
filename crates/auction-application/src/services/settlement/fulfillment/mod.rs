//! 结算履约应用服务的模块装配。

mod authorization;
mod canonical;
mod collected;
mod evidence;
mod intent;
mod internal;
mod model;
mod proof;
mod refund;
mod schedule;
mod service;
mod validation;

pub use service::SettlementFulfillmentService;
