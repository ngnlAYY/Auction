//! 连接用途闭集；名称不得来自用户、凭据或数据库名称。

/// 服务与运维入口显式选择的用途。
#[derive(Debug, Clone, Copy)]
pub enum DatabaseConnectionPurpose {
    Public,
    Admin,
    Worker,
    CliStatus,
    CliMigrate,
    CliPreflight,
    CliInventory,
    CliRetentionDryRun,
    CliBackupCheck,
    CliBackup,
    CliRestore,
    CliRebuild,
    CliCreate,
    CliOperations,
    CliServiceCheck,
    ServerPreflight,
    ServerInitialize,
    SetupCheck,
}

impl DatabaseConnectionPurpose {
    /// 固定 application_name；包含连字符的 ASCII 名称均小于 PostgreSQL 标识长度。
    pub const fn application_name(self) -> &'static str {
        match self {
            Self::Public => "auction-public",
            Self::Admin => "auction-admin",
            Self::Worker => "auction-worker",
            Self::CliStatus => "auction-cli-status",
            Self::CliMigrate => "auction-cli-migrate",
            Self::CliPreflight => "auction-cli-preflight",
            Self::CliInventory => "auction-cli-inventory",
            Self::CliRetentionDryRun => "auction-cli-retention-dry-run",
            Self::CliBackupCheck => "auction-cli-backup-check",
            Self::CliBackup => "auction-cli-backup",
            Self::CliRestore => "auction-cli-restore",
            Self::CliRebuild => "auction-cli-rebuild",
            Self::CliCreate => "auction-cli-create",
            Self::CliOperations => "auction-cli-operations",
            Self::CliServiceCheck => "auction-cli-service-check",
            Self::ServerPreflight => "auction-server-preflight",
            Self::ServerInitialize => "auction-server-initialize",
            Self::SetupCheck => "auction-setup-check",
        }
    }
}
