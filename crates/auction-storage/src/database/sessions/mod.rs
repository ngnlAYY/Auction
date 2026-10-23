//! Admin 与 Public 会话分别由子模块持有；common 只提供存储错误映射。

use auction_application::{
    AdminSessionIdentity, AdminSessionRecord, AdminSessionStore, ApplicationFuture,
    PublicSessionIdentity, PublicSessionPreferenceResult, PublicSessionRecord, PublicSessionStore,
    SetPublicSessionPreferenceCommand,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

mod admin;
mod common;
mod public;

impl AdminSessionStore for DatabaseStore {
    fn revoke_all_admin(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => admin::revoke_all_postgres(store, now).await,
            }
        })
    }

    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => admin::create_postgres(store, record).await,
            }
        })
    }

    fn touch_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    admin::touch_postgres(store, token_hash, csrf_hash, now, next_expires_at).await
                }
            }
        })
    }

    fn revoke_admin(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    admin::revoke_postgres(store, token_hash, now).await
                }
            }
        })
    }
}

impl PublicSessionStore for DatabaseStore {
    fn inspect_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::inspect_postgres(store, token_hash, now).await
                }
            }
        })
    }

    fn create_public(
        &self,
        record: PublicSessionRecord,
        max_public_sessions: u32,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::create_postgres(store, record, max_public_sessions).await
                }
            }
        })
    }

    fn touch_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::touch_postgres(store, token_hash, now).await
                }
            }
        })
    }

    fn revoke_public(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::revoke_postgres(store, token_hash, now).await
                }
            }
        })
    }

    fn get_public_preference(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::get_preference_postgres(store, token_hash, now).await
                }
            }
        })
    }

    fn set_public_preference(
        &self,
        token_hash: [u8; 32],
        command: SetPublicSessionPreferenceCommand,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "postgres")]
                DatabaseStore::Postgres(store) => {
                    public::set_preference_postgres(store, token_hash, command, now).await
                }
            }
        })
    }
}
