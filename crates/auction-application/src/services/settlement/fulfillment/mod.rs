//! 结算履约应用服务的模块装配。

mod activate;
mod authorization;
mod canonical;
mod collected;
mod evidence;
mod intent;
mod internal;
mod model;
mod outcome;
mod prepare;
mod proof;
mod reconcile;
mod refund;
mod schedule;
mod service;
mod snapshot;
mod validation;

pub use service::SettlementFulfillmentService;
