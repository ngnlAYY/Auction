//! MoogleMail 协议模型、URL 构造和输入校验的稳定出口。

mod builder;
mod model;
mod validation;

const PRODUCTION_HOST: &str = "hentaiverse.org";
const MAX_COOKIE_BYTES: usize = 4096;
const MAX_TOKEN_BYTES: usize = 1024;
const MAX_OPAQUE_ID_BYTES: usize = 256;

pub(crate) use builder::MoogleMailUrlBuilder;
pub use model::{
    MoogleMailCookie, MoogleMailEquipmentKey, MoogleMailOperation, MoogleMailTarget,
    MoogleMailToken, MoogleMailWorld,
};
pub(crate) use validation::{valid_decimal_id, valid_hex_key, valid_line, valid_opaque_id};
