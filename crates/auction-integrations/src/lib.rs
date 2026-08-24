//! 外部服务适配器的薄 crate 入口。

mod clients;
mod credentials;
mod cryptobox;
mod equipment;
mod forum;
mod login_key;

pub use clients::{build_http_client, ehentai, mooglemail};
pub use credentials::CredentialSealer;
pub use cryptobox::{DataKey, EquipmentKeySealer, EquipmentKeySealerError};
pub use equipment::{
    EquipmentSourceAdapter, EquipmentSourceConfigurationError, build_equipment_http_client,
};
pub use forum::{
    ForumPreviewAdapter, ForumRenderError, ForumThreadRenderer, render_auction_thread,
};
pub use login_key::{LoginKeyCryptoError, MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader};
