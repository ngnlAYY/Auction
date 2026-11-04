//! 外部服务适配器的薄 crate 入口。

#[cfg(feature = "backup")]
mod backup;
mod clients;
mod credentials;
mod cryptobox;
mod diagnostics;
mod equipment;
mod forum;
mod login_key;
mod worker_control;

#[cfg(feature = "backup")]
pub use backup::{
    BackupHeartbeatResult, BackupHeartbeatStatus, BackupObject, BackupObjectStatus,
    BackupStoreError, R2BackupStore, send_backup_heartbeat,
};
pub use clients::{ehentai, mooglemail};
pub use credentials::{CredentialAcquirer, CredentialSealer, CredentialVerifier};
pub use cryptobox::{
    DataKey, EquipmentKeyOpener, EquipmentKeyOpenerError, EquipmentKeySealer,
    EquipmentKeySealerError,
};
pub use equipment::{
    EquipmentGameLinkResolver, EquipmentSourceAdapter, EquipmentSourceConfigurationError,
    EquipmentSourcePolicy, WorkerEquipmentSourceReader, build_equipment_http_client,
    canonical_json_snapshot,
};
pub use forum::{ForumPreviewAdapter, ForumRenderError, ForumThreadRenderer};
pub use login_key::{LoginKeyCryptoError, MoogleMailLoginKeyIssuer, MoogleMailLoginKeyReader};
pub use worker_control::{WorkerRemoteReadClient, WorkerRemoteReadClientError};
