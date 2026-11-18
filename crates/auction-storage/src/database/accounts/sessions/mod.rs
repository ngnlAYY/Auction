//! Admin 与 Public 会话分别由子模块持有。

use auction_application::core::ApplicationFuture;
use auction_application::public::{
    AdminSessionIdentity, AdminSessionRecord, AdminSessionStore, PublicSessionIdentity,
    PublicSessionPreferenceResult, PublicSessionRecord, PublicSessionStore,
    SetPublicSessionPreferenceCommand,
};
use auction_time::Timestamp;

use crate::database::DatabaseStore;

mod admin;
mod public;

impl AdminSessionStore for DatabaseStore {
    fn revoke_all_admin(&self, now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { admin::revoke_all_postgres(self, now).await })
    }

    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move { admin::create_postgres(self, record).await })
    }

    fn inspect_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            admin::authenticate_postgres(self, token_hash, csrf_hash, now, None).await
        })
    }

    fn touch_admin(
        &self,
        token_hash: [u8; 32],
        csrf_hash: [u8; 32],
        now: Timestamp,
        next_expires_at: Timestamp,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            admin::authenticate_postgres(self, token_hash, csrf_hash, now, Some(next_expires_at))
                .await
        })
    }

    fn revoke_admin(&self, token_hash: [u8; 32], now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { admin::revoke_postgres(self, token_hash, now).await })
    }
}

impl PublicSessionStore for DatabaseStore {
    fn inspect_public(
        &self,
        token_hash: [u8; 32],
        now: Timestamp,
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
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionIdentity> {
        Box::pin(async move { public::touch_postgres(self, token_hash, now).await })
    }

    fn revoke_public(&self, token_hash: [u8; 32], now: Timestamp) -> ApplicationFuture<'_, ()> {
        Box::pin(async move { public::revoke_postgres(self, token_hash, now).await })
    }

    fn get_public_preference(
        &self,
        token_hash: [u8; 32],
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(async move { public::get_preference_postgres(self, token_hash, now).await })
    }

    fn set_public_preference(
        &self,
        token_hash: [u8; 32],
        command: SetPublicSessionPreferenceCommand,
        now: Timestamp,
    ) -> ApplicationFuture<'_, PublicSessionPreferenceResult> {
        Box::pin(
            async move { public::set_preference_postgres(self, token_hash, command, now).await },
        )
    }
}
