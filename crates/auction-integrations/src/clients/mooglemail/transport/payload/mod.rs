//! MoogleMail delivery payload 的分型校验入口。

mod consignment;
mod mail;
mod settlement;

pub use consignment::{
    DownstreamDispatchFacts, FirstDispatchFacts, RenderedDispatchMail, WithdrawalDispatchFacts,
    render_consignment_send, render_downstream_dispatch, render_first_dispatch,
    render_frozen_dispatch, render_withdrawal_dispatch,
};
pub use mail::decode_consignment_disposition_request;
pub(crate) use mail::{decode_payload, is_notification_origin};
pub(crate) use settlement::{decode_settlement_payload, render_settlement};
