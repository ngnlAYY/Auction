//! 外部服务适配器的薄 crate 入口。

#[cfg(feature = "backup")]
mod backup;
mod clients;
mod credentials;
mod cryptobox;
mod diagnostics;
mod equipment;
mod forum;
mod heartbeat;
mod keyring;
mod login_key;
mod operations_alert;
mod worker_control;

#[cfg(feature = "backup")]
pub use backup::{BackupObject, BackupObjectStatus, BackupStoreError, R2BackupStore};
pub use clients::{ehentai, mooglemail};
pub use credentials::{CredentialAcquirer, CredentialSealer, CredentialVerifier};
pub use cryptobox::{
    DataKey, EquipmentKeyOpener, EquipmentKeyOpenerError, EquipmentKeySealer,
    EquipmentKeySealerError,
};
pub use equipment::{
    EquipmentGameLinkResolver, EquipmentSourceAdapter, EquipmentSourcePolicy,
    WorkerEquipmentSourceReader, build_equipment_http_client, canonical_json_snapshot,
};
pub use forum::{ForumPreviewAdapter, ForumRenderError, ForumThreadRenderer};
pub use heartbeat::{BackupHeartbeatResult, BackupHeartbeatStatus, send_heartbeat};
pub use keyring::{Keyring, KeyringError, build_keyring};
pub use login_key::{MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader};
pub use operations_alert::{OperationsAlertWebhook, OperationsAlertWebhookConfigError};
pub use worker_control::{WorkerRemoteReadClient, WorkerRemoteReadClientError};
