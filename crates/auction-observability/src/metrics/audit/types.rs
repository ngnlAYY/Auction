//! 审计指标的闭集 label；不接收 SQL、驱动原文或业务值。

/// 审计失败的就绪风险分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFailureClass {
    Transient,
    Permanent,
    Unknown,
}

impl AuditFailureClass {
    pub const ALL: [Self; 3] = [Self::Transient, Self::Permanent, Self::Unknown];

    /// 返回编译期固定的指标与日志 label。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Permanent => "permanent",
            Self::Unknown => "unknown",
        }
    }
}

/// SQLSTATE 的已知类别；非 SQL 错误用 none，其余类别归 other。
#[derive(Debug, Clone, Copy)]
pub enum AuditSqlstateClass {
    None,
    Connection,
    TriggeredAction,
    Cardinality,
    Data,
    Constraint,
    Transaction,
    TriggeredChange,
    Authorization,
    Routine,
    Catalog,
    Schema,
    Rollback,
    Sql,
    WithCheck,
    Resources,
    Prerequisite,
    Intervention,
    System,
    Plpgsql,
    Internal,
    Other,
}

impl AuditSqlstateClass {
    pub const ALL: [Self; 22] = [
        Self::None,
        Self::Connection,
        Self::TriggeredAction,
        Self::Cardinality,
        Self::Data,
        Self::Constraint,
        Self::Transaction,
        Self::TriggeredChange,
        Self::Authorization,
        Self::Routine,
        Self::Catalog,
        Self::Schema,
        Self::Rollback,
        Self::Sql,
        Self::WithCheck,
        Self::Resources,
        Self::Prerequisite,
        Self::Intervention,
        Self::System,
        Self::Plpgsql,
        Self::Internal,
        Self::Other,
    ];

    /// 返回安全类别；不输出任意服务端 SQLSTATE。
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Connection => "08",
            Self::TriggeredAction => "09",
            Self::Cardinality => "21",
            Self::Data => "22",
            Self::Constraint => "23",
            Self::Transaction => "25",
            Self::TriggeredChange => "27",
            Self::Authorization => "28",
            Self::Routine => "39",
            Self::Catalog => "3D",
            Self::Schema => "3F",
            Self::Rollback => "40",
            Self::Sql => "42",
            Self::WithCheck => "44",
            Self::Resources => "53",
            Self::Prerequisite => "55",
            Self::Intervention => "57",
            Self::System => "58",
            Self::Plpgsql => "P0",
            Self::Internal => "XX",
            Self::Other => "other",
        }
    }
}

/// 审计错误的安全 kind；分类由固定变体决定，不携带错误原文。
#[derive(Debug, Clone, Copy)]
pub enum AuditFailureKind {
    SerializationConflict,
    Deadlock,
    QueryCanceled,
    LockUnavailable,
    ConnectionException,
    PoolTimedOut,
    ConnectionIo,
    ConstraintViolation,
    TriggerRejected,
    PermissionDenied,
    AuthenticationRejected,
    SchemaMismatch,
    InvalidAuditData,
    DriverConfiguration,
    DriverTls,
    DriverInvalidArgument,
    DriverProtocol,
    DriverData,
    DriverState,
    UnknownSqlstate,
    UnknownDriver,
    ConnectionCapacity,
    ServerShutdown,
    ServerCrash,
    ServerStarting,
}

impl AuditFailureKind {
    pub const ALL: [Self; 25] = [
        Self::SerializationConflict,
        Self::Deadlock,
        Self::QueryCanceled,
        Self::LockUnavailable,
        Self::ConnectionException,
        Self::PoolTimedOut,
        Self::ConnectionIo,
        Self::ConstraintViolation,
        Self::TriggerRejected,
        Self::PermissionDenied,
        Self::AuthenticationRejected,
        Self::SchemaMismatch,
        Self::InvalidAuditData,
        Self::DriverConfiguration,
        Self::DriverTls,
        Self::DriverInvalidArgument,
        Self::DriverProtocol,
        Self::DriverData,
        Self::DriverState,
        Self::UnknownSqlstate,
        Self::UnknownDriver,
        Self::ConnectionCapacity,
        Self::ServerShutdown,
        Self::ServerCrash,
        Self::ServerStarting,
    ];

    /// 返回固定 kind 对应的风险分类；unknown 是保守风险处置。
    pub const fn class(self) -> AuditFailureClass {
        match self {
            Self::SerializationConflict
            | Self::Deadlock
            | Self::QueryCanceled
            | Self::LockUnavailable
            | Self::ConnectionException
            | Self::PoolTimedOut
            | Self::ConnectionIo
            | Self::ConnectionCapacity
            | Self::ServerShutdown
            | Self::ServerCrash
            | Self::ServerStarting => AuditFailureClass::Transient,
            Self::DriverTls
            | Self::DriverProtocol
            | Self::UnknownSqlstate
            | Self::UnknownDriver => AuditFailureClass::Unknown,
            _ => AuditFailureClass::Permanent,
        }
    }

    /// 返回编译期固定的指标与日志 label。
    pub const fn label(self) -> &'static str {
        match self {
            Self::SerializationConflict => "serialization_conflict",
            Self::Deadlock => "deadlock",
            Self::QueryCanceled => "query_canceled",
            Self::LockUnavailable => "lock_unavailable",
            Self::ConnectionException => "connection_exception",
            Self::PoolTimedOut => "pool_timed_out",
            Self::ConnectionIo => "connection_io",
            Self::ConstraintViolation => "constraint_violation",
            Self::TriggerRejected => "trigger_or_routine_rejected",
            Self::PermissionDenied => "permission_denied",
            Self::AuthenticationRejected => "authentication_rejected",
            Self::SchemaMismatch => "schema_or_sql_mismatch",
            Self::InvalidAuditData => "invalid_audit_data",
            Self::DriverConfiguration => "driver_configuration",
            Self::DriverTls => "driver_tls",
            Self::DriverInvalidArgument => "driver_invalid_argument",
            Self::DriverProtocol => "driver_protocol",
            Self::DriverData => "driver_data",
            Self::DriverState => "driver_state",
            Self::UnknownSqlstate => "unknown_sqlstate",
            Self::UnknownDriver => "unknown_driver",
            Self::ConnectionCapacity => "connection_capacity",
            Self::ServerShutdown => "server_shutdown",
            Self::ServerCrash => "server_crash",
            Self::ServerStarting => "server_starting",
        }
    }
}

/// 一种固定 label 组合的进程累计失败数。
pub struct AuditFailureObservation {
    pub kind: AuditFailureKind,
    pub sqlstate_class: AuditSqlstateClass,
    pub count: i64,
}

/// 不访问数据库的审计安全快照。
pub struct AuditMetricsSnapshot {
    pub failures: Vec<AuditFailureObservation>,
    pub readiness_blocked: bool,
    pub shadow_mismatches: [i64; 3],
}

/// 存储适配器提供进程计数；指标关闭不改变其采集与就绪门禁。
pub trait AuditMetricsSource: Send + Sync {
    fn snapshot(&self) -> AuditMetricsSnapshot;
}
