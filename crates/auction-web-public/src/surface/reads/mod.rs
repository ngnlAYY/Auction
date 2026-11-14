//! Public 只读历史页面的职责分组。

use super::{supported_worlds, world_label};

mod bid_log;
mod history;

pub(crate) use bid_log::bid_log;
pub(crate) use history::{history, history_equipment_name_query};
