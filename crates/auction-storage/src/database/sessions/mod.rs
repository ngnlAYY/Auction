//! Admin 与 Public 会话分别由子模块持有。

use auction_application::auction::ApplicationFuture;
use auction_application::public::{
    AdminSessionIdentity, AdminSessionRecord, AdminSessionStore, PublicSessionIdentity,
    PublicSessionPreferenceResult, PublicSessionRecord, PublicSessionStore,
    SetPublicSessionPreferenceCommand,
};
use chrono::{DateTime, Utc};

use super::DatabaseStore;

mod admin;
mod public;

pub(super) use crate::database::storage_error;

impl AdminSessionStore for DatabaseStore {
    fn revoke_all_admin(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { admin::revoke_all_postgres(self, now).await })
    }

    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move { admin::create_postgres(self, record).await })
    }

    fn touch_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: DateTime<Utc>,
        next_expires_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            admin::touch_postgres(self, token_hash, csrf_hash, now, next_expires_at).await
        })
    }

    fn revoke_admin(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { admin::revoke_postgres(self, token_hash, now).await })
    }
}

impl PublicSessionStore for DatabaseStore {
    fn inspect_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move { public::inspect_postgres(self, token_hash, now).await })
    }

    fn create_public(
        &self,
        record: PublicSessionRecord,
        max_public_sessions: u32,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move { public::create_postgres(self, record, max_public_sessions).await })
    }

    fn touch_public(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move { public::touch_postgres(self, token_hash, now).await })
    }

    fn revoke_public(&self, token_hash: [u8; 32], now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { public::revoke_postgres(self, token_hash, now).await })
    }

    fn get_public_preference(
        &self,
        token_hash: [u8; 32],
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move { public::get_preference_postgres(self, token_hash, now).await })
    }

    fn set_public_preference(
        &self,
        token_hash: [u8; 32],
        command: SetPublicSessionPreferenceCommand,
        now: DateTime<Utc>,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(
            async move { public::set_preference_postgres(self, token_hash, command, now).await },
        )
    }
}
