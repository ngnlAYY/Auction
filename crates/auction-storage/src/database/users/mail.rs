//! Users owner 的 MoogleMail 注册、换绑和登录 key 密文读取。
//!
//! 跨后端端口分派只保留在本文件；PostgreSQL 完整档的注册/换绑命令和幂等账本、SQLite Lite
//! 的信封读取分别由子模块拥有。注册与换绑仍只在 PostgreSQL 完整档提供。

#[cfg(feature = "postgres")]
use auction_application::MailUserResultCode;
use auction_application::{
    ApplicationError, MailLoginKeyEnvelope, MailLoginKeyEnvelopeReader, MailUserCommandKind,
    MailUserRebindCommand, MailUserRegisterWrite, MailUserResult, MailUserStore, RepositoryError,
    RequestHash,
};
use auction_domain::CommandId;
#[cfg(feature = "postgres")]
use chrono::{DateTime, Utc};
#[cfg(feature = "postgres")]
use serde_json::json;
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::PgRow;
#[cfg(feature = "postgres")]
use subtle::ConstantTimeEq;
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[path = "mail/postgres.rs"]
mod postgres;
#[cfg(feature = "sqlite")]
#[path = "mail/sqlite.rs"]
mod sqlite;

use super::DatabaseStore;

impl MailUserStore for DatabaseStore {
    fn find_mail_command(
        &self,
        _command_id: CommandId,
        _kind: MailUserCommandKind,
        _request_hash: RequestHash,
    ) -> auction_application::ApplicationFuture<'_, Option<MailUserResult>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    postgres::find_mail_command_postgres(store, _command_id, _kind, _request_hash)
                        .await
                }
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 命令 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn register_or_recover(
        &self,
        _write: MailUserRegisterWrite,
    ) -> auction_application::ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::register_postgres(store, _write).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 注册 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }

    fn rebind_game_identity(
        &self,
        _command: MailUserRebindCommand,
    ) -> auction_application::ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::rebind_postgres(store, _command).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(_) => Err(ApplicationError::CapabilityUnavailable(
                    "Users MoogleMail 换绑 owner 仅支持 PostgreSQL 完整档",
                )),
            }
        })
    }
}

impl MailLoginKeyEnvelopeReader for DatabaseStore {
    fn load_login_key_envelope(
        &self,
        user_id: Uuid,
    ) -> auction_application::ApplicationFuture<'_, MailLoginKeyEnvelope> {
        Box::pin(async move {
            if user_id.is_nil() {
                return Err(repository_error("登录 key 信封用户标识无效"));
            }
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::load_envelope_postgres(store, user_id).await,
                #[cfg(feature = "sqlite")]
                Self::Sqlite(store) => sqlite::load_envelope_sqlite(store, user_id).await,
            }
        })
    }
}

fn repository_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
