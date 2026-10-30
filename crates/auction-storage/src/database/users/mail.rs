//! 游戏邮件注册、换绑的 PostgreSQL 端口分派；命令与幂等账本由子模块拥有。

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
mod postgres;

use super::DatabaseStore;

impl MailUserStore for DatabaseStore {
    fn find_registration_user_id(
        &self,
        _username: String,
    ) -> auction_application::ApplicationFuture<'_, Option<Uuid>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => {
                    postgres::find_registration_user_id(store, &_username).await
                }
            }
        })
    }

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
            }
        })
    }

    fn rebind_username(
        &self,
        _command: MailUserRebindCommand,
    ) -> auction_application::ApplicationFuture<'_, MailUserResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                Self::Postgres(store) => postgres::rebind_postgres(store, _command).await,
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
            }
        })
    }
}

fn repository_error(message: &str) -> ApplicationError {
    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
}
