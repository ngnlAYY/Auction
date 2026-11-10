//! 原始 sqlx 错误的审计风险分类；只匹配固定代码和变体，绝不读取 message/detail。

use auction_observability::{AuditFailureKind as Kind, AuditSqlstateClass as Class};

pub(super) fn classify(error: &sqlx::Error) -> (Kind, Class) {
    match error {
        sqlx::Error::Database(database) => database
            .code()
            .map_or((Kind::UnknownSqlstate, Class::None), |code| classify_sqlstate(&code)),
        sqlx::Error::PoolTimedOut => (Kind::PoolTimedOut, Class::None),
        sqlx::Error::Io(_) => (Kind::ConnectionIo, Class::None),
        sqlx::Error::Configuration(_) | sqlx::Error::ConfigFile(_) => {
            (Kind::DriverConfiguration, Class::None)
        }
        sqlx::Error::InvalidArgument(_) => (Kind::DriverInvalidArgument, Class::None),
        sqlx::Error::Tls(_) => (Kind::DriverTls, Class::None),
        sqlx::Error::Protocol(_) => (Kind::DriverProtocol, Class::None),
        sqlx::Error::RowNotFound
        | sqlx::Error::TypeNotFound { .. }
        | sqlx::Error::ColumnIndexOutOfBounds { .. }
        | sqlx::Error::ColumnNotFound(_)
        | sqlx::Error::ColumnDecode { .. }
        | sqlx::Error::Encode(_)
        | sqlx::Error::Decode(_) => (Kind::DriverData, Class::None),
        sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed
        | sqlx::Error::InvalidSavePointStatement => (Kind::DriverState, Class::None),
        // 未知风险处置，不代表已证明不可重试；包括 BeginFailed、Migrate 与未来驱动变体。
        _ => (Kind::UnknownDriver, Class::None),
    }
}

fn classify_sqlstate(code: &str) -> (Kind, Class) {
    if code.len() != 5
        || !code.bytes().all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
    {
        return (Kind::UnknownSqlstate, Class::Other);
    }
    let class = match &code[..2] {
        "08" => Class::Connection,
        "09" => Class::TriggeredAction,
        "21" => Class::Cardinality,
        "22" => Class::Data,
        "23" => Class::Constraint,
        "25" => Class::Transaction,
        "27" => Class::TriggeredChange,
        "28" => Class::Authorization,
        "39" => Class::Routine,
        "3D" => Class::Catalog,
        "3F" => Class::Schema,
        "40" => Class::Rollback,
        "42" => Class::Sql,
        "44" => Class::WithCheck,
        "53" => Class::Resources,
        "55" => Class::Prerequisite,
        "57" => Class::Intervention,
        "58" => Class::System,
        "P0" => Class::Plpgsql,
        "XX" => Class::Internal,
        _ => Class::Other,
    };
    let kind = match code {
        "40001" => Kind::SerializationConflict,
        "40P01" => Kind::Deadlock,
        "57014" => Kind::QueryCanceled,
        "55P03" => Kind::LockUnavailable,
        "53300" => Kind::ConnectionCapacity,
        "57P01" => Kind::ServerShutdown,
        "57P02" => Kind::ServerCrash,
        "57P03" => Kind::ServerStarting,
        "42501" => Kind::PermissionDenied,
        "55000" => Kind::TriggerRejected,
        _ => match class {
            // 整个 08 类按决策 32 放行健康门禁；08007 等结果未知仍禁止自动重放事务。
            Class::Connection => Kind::ConnectionException,
            Class::Constraint => Kind::ConstraintViolation,
            Class::TriggeredAction | Class::TriggeredChange | Class::Plpgsql => {
                Kind::TriggerRejected
            }
            Class::Routine if matches!(code, "39P01" | "39P02" | "39P03") => Kind::TriggerRejected,
            Class::Authorization => Kind::AuthenticationRejected,
            Class::Sql | Class::Catalog | Class::Schema => Kind::SchemaMismatch,
            Class::Cardinality | Class::Data | Class::WithCheck => Kind::InvalidAuditData,
            // 依决策 71 加入 53300、57P01–57P03；57P05、25P03/25P04 仍按未知处理。
            // 未知风险处置，不代表已证明不可重试。
            _ => Kind::UnknownSqlstate,
        },
    };
    (kind, class)
}
