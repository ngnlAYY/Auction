//! MoogleMail HTML 文档解析器的职责分组。

/// 无状态的 MoogleMail HTML 解析器。
#[derive(Debug, Clone, Copy, Default)]
pub struct MoogleMailParser;

mod actions;
mod consignment;
mod detail;
mod list_helpers;
mod primitives;
mod reads;
mod script;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use actions::parse_detail_actions_lenient;
pub(crate) use actions::returned_take_state;
pub use consignment::{
    CONSIGNMENT_PARSER_VERSION, ConsignmentAttachmentObservation, ConsignmentDetailObservation,
    ConsignmentListRow, FrozenMailAttachment, FrozenMailManifest, TakeReceiptObservation,
    observe_take_receipt, parse_consignment_detail, parse_consignment_list_row,
};
pub(crate) use detail::*;
pub(crate) use list_helpers::*;
pub(crate) use primitives::*;
pub(crate) use reads::MoogleMailInventoryPreparation;
pub(crate) use script::*;
pub use types::*;
