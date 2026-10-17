//! 结算履约应用服务的模块装配。
//!
//! 服务编排、delivery intent、阶段/证据校验和 proof 摘要由各自子模块拥有；本模块只保留
//! 稳定出口，避免把外部 MoogleMail 传输细节带入结算端口。

mod authorization;
mod canonical;
mod collected;
mod evidence;
mod intent;
mod internal;
mod model;
mod proof;
mod refund;
mod service;
mod validation;

pub use service::SettlementFulfillmentService;
