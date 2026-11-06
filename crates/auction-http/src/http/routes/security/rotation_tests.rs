//! 用本地会话存储替身核对共享 Admin 凭据轮换；不连接数据库或账号。

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use auction_application::auction::ApplicationFuture;
use auction_application::core::SessionError;
use auction_application::public::{
    AdminSessionConfig, AdminSessionIdentity, AdminSessionPort, AdminSessionRecord,
    AdminSessionStore,
};
use auction_application::sessions::AdminSessionService;
use chrono::{DateTime, Duration, Utc};

#[derive(Default)]
struct MemorySessions(Mutex<HashMap<[u8; 32], AdminSessionRecord>>);

impl AdminSessionStore for MemorySessions {
    fn revoke_all_admin(&self, _now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().clear();
            Ok(())
        })
    }

    fn create_admin(
        &self,
        record: AdminSessionRecord,
    ) -> ApplicationFuture<'_, AdminSessionIdentity> {
        Box::pin(async move {
            let identity = identity(&record);
            self.0.lock().unwrap().insert(record.token_hash, record);
            Ok(identity)
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
            let mut rows = self.0.lock().unwrap();
            let row = rows.get_mut(&token_hash).ok_or(SessionError::Unauthenticated)?;
            if row.csrf_hash != csrf_hash || row.expires_at <= now || row.absolute_expires_at <= now
            {
                return Err(SessionError::Unauthenticated.into());
            }
            row.expires_at = next_expires_at.min(row.absolute_expires_at);
            Ok(identity(row))
        })
    }

    fn revoke_admin(&self, token_hash: [u8; 32], _now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().remove(&token_hash);
            Ok(())
        })
    }
}

fn identity(record: &AdminSessionRecord) -> AdminSessionIdentity {
    AdminSessionIdentity {
        actor: record.actor.clone(),
        expires_at: record.expires_at,
        absolute_expires_at: record.absolute_expires_at,
    }
}

async fn service(store: Arc<MemorySessions>, secret: u8) -> AdminSessionService {
    AdminSessionService::new(
        store,
        "operator".to_owned(),
        vec![secret; 32],
        AdminSessionConfig { idle_ttl: Duration::minutes(15), absolute_ttl: Duration::hours(8) },
        Utc::now(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn rotating_secret_rejects_old_login_and_never_revives_old_sessions() {
    let store = Arc::new(MemorySessions::default());
    let first = service(store.clone(), b'a').await;
    let old = first.login("operator".to_owned(), vec![b'a'; 32], Utc::now()).await.unwrap();
    assert!(first.current(old.token.clone(), old.csrf_token.clone(), Utc::now()).await.is_ok());
    drop(first);

    let rotated = service(store.clone(), b'b').await;
    assert!(rotated.login("operator".to_owned(), vec![b'a'; 32], Utc::now()).await.is_err());
    assert!(rotated.current(old.token.clone(), old.csrf_token.clone(), Utc::now()).await.is_err());
    let new = rotated.login("operator".to_owned(), vec![b'b'; 32], Utc::now()).await.unwrap();
    assert_eq!(new.identity.actor, old.identity.actor);
    assert!(rotated.current(new.token.clone(), new.csrf_token.clone(), Utc::now()).await.is_ok());
    drop(rotated);

    let rollback = service(store, b'a').await;
    assert!(rollback.current(old.token, old.csrf_token, Utc::now()).await.is_err());
    assert!(rollback.current(new.token, new.csrf_token, Utc::now()).await.is_err());
    assert!(rollback.login("operator".to_owned(), vec![b'a'; 32], Utc::now()).await.is_ok());
}
