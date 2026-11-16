//! Public 只读历史页面的职责分组。

mod bid_log;
mod fulfillment;
mod history;

pub(crate) use bid_log::bid_log;
pub(crate) use fulfillment::fulfillment;
pub(crate) use history::{history, history_equipment_name_query};
