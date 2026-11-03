//! 备份的外部传输出口；打卡客户端与游戏客户端、Cookie 和重试策略隔离。

mod heartbeat;
mod r2;

pub use heartbeat::{BackupHeartbeatResult, BackupHeartbeatStatus, send_backup_heartbeat};
pub use r2::{BackupObject, BackupObjectStatus, BackupStoreError, R2BackupStore};
