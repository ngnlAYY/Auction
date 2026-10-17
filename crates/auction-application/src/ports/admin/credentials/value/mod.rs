//! Admin 凭据值对象的稳定出口。

mod common;
mod error;
mod health;
mod model;
mod runtime;
mod secret;

pub use common::{
    MAX_ADMIN_CREDENTIAL_OFFSET, MAX_ADMIN_CREDENTIAL_PAGE_SIZE, MAX_ADMIN_CREDENTIAL_SECRET_BYTES,
};
pub use error::*;
pub use health::*;
pub use model::*;
pub use runtime::*;
pub use secret::*;
