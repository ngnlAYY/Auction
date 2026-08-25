//! MoogleMail delivery payload 的分型校验入口。

mod mail;
mod settlement;

pub(crate) use mail::decode_payload;
pub(crate) use settlement::{decode_settlement_payload, render_settlement};
