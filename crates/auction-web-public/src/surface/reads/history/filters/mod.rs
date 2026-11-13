//! Public 价格历史筛选器的装配与稳定导出。
//! 筛选表单、输入模型和规范化校验分别归档到子模块；`history` 页面继续通过本模块的
//! `pub(crate)` 导出使用原有组件与类型。

mod components;
mod model;
mod validation;

pub(crate) use components::PriceHistoryFilters;
pub(crate) use model::{HistoryDateError, HistoryFilterError, HistoryPriceError, HistoryPriceUnit};
pub(crate) use validation::{
    history_date_error, history_date_value, history_equipment_name_query, history_month_label,
    history_name_error, history_name_value, history_price_error, history_price_value,
};
