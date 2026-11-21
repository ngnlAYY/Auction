//! 外部服务适配器的薄 crate 入口。

// 测试里的断言直接解包；该豁免只在测试构建生效，生产构建仍按 workspace lint 拒绝 unwrap/expect。
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "backup")]
mod backup;
mod clients;
mod credentials;
mod cryptobox;
mod equipment;
mod forum;
mod heartbeat;
mod html;
mod keyring;
mod login_key;
mod operations_alert;
mod outbound;
mod safe_diagnostics;
mod worker_control;

#[cfg(feature = "backup")]
pub use backup::{BackupObject, BackupObjectStatus, BackupStoreError, R2BackupStore};
pub use clients::{ehentai, mooglemail};
pub use credentials::{
    CredentialAcquirer, CredentialSealer, CredentialVerifier, WorkerMailCredentialVerifier,
};
pub use cryptobox::{
    DataKey, EquipmentKeyOpener, EquipmentKeyOpenerError, EquipmentKeySealer,
    EquipmentKeySealerError,
};
pub use equipment::hvdata::{HvDataConfigurationError, HvDataEquipmentReader};
pub use equipment::{
    EquipmentGameLinkResolver, EquipmentSourceAdapter, EquipmentSourcePolicy,
    HentaiVerseListingUrlValidator, WorkerEquipmentSourceReader, build_equipment_http_client,
};
pub use forum::{ForumRenderError, ForumThreadRenderer};
pub use heartbeat::{
    BackupHeartbeat, BackupHeartbeatResult, BackupHeartbeatStatus, send_heartbeat,
};
pub use keyring::{Keyring, KeyringError, build_keyring};
pub use login_key::{MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader};
pub use operations_alert::{OperationsAlertWebhook, OperationsAlertWebhookConfigError};
pub use worker_control::{
    WorkerControlClient, WorkerControlClientBuildError, WorkerControlClientError,
    WorkerRemoteReadClient, WorkerRemoteReadClientError,
};
