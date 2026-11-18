//! Public Cookie 写入的原生锁与常量规模同步提示；提示不持有 token，也不决定认证或服务器期限。

use auction_web_shared::{BrowserLock, BrowserLockError, DEFAULT_REQUEST_TIMEOUT_MILLIS};

use super::PublicSession;
#[cfg(test)]
use super::transport::SessionHttpFixture;

const LOCK_NAME: &str = "hv.public.session-cookie";

/// 本次会话请求持有的独占锁，直到完整响应或传输取消。
pub(in crate::api) struct CookieLock {
    _native: Option<BrowserLock>,
    #[cfg(test)]
    track_fixture: Option<fn(bool)>,
}

impl Drop for CookieLock {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(track) = self.track_fixture {
            track(false);
        }
    }
}

/// 登录、退出、偏好命令和浏览器持久期同步共用同一原生独占锁。
pub(in crate::api) async fn lock() -> Result<CookieLock, &'static str> {
    #[cfg(test)]
    if let Some(fixture) = dioxus::prelude::try_consume_context::<SessionHttpFixture>() {
        (fixture.wait_for_lock)().await;
        (fixture.track_lock)(true);
        return Ok(CookieLock { _native: None, track_fixture: Some(fixture.track_lock) });
    }
    let native =
        BrowserLock::acquire(LOCK_NAME, DEFAULT_REQUEST_TIMEOUT_MILLIS).await.map_err(|error| {
            match error {
                BrowserLockError::Unavailable => "session_lock_unavailable",
                BrowserLockError::Timeout => "session_lock_timeout",
            }
        })?;
    Ok(CookieLock {
        _native: Some(native),
        #[cfg(test)]
        track_fixture: None,
    })
}

#[cfg(target_arch = "wasm32")]
const SYNC_INTERVAL_MILLIS: f64 = 24.0 * 60.0 * 60.0 * 1_000.0;

#[cfg(target_arch = "wasm32")]
struct SyncedCookie {
    user_id: String,
    expires_at: Option<String>,
    absolute_expires_at: Option<String>,
    synchronized_at: f64,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static SYNCED_COOKIE: std::cell::RefCell<Option<SyncedCookie>> = const { std::cell::RefCell::new(None) };
}

/// 首次或期限变化须同步；其余在下一次权威读取且距上次同步满一天时刷新浏览器持久期。
/// 此判断只在本次权威 GET 已认证后调用；提示丢失只增加一次显式同步，不能授权用户。
pub(super) fn needs_sync(session: &PublicSession) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::now();
        SYNCED_COOKIE.with_borrow(|synced| {
            synced.as_ref().is_none_or(|synced| {
                synced.user_id != session.user_id
                    || synced.expires_at != session.expires_at
                    || synced.absolute_expires_at != session.absolute_expires_at
                    || !now.is_finite()
                    || now < synced.synchronized_at
                    || now - synced.synchronized_at >= SYNC_INTERVAL_MILLIS
            })
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = session;
        true
    }
}

/// 只在完整成功响应及安全投影解码后记录；网络失败或组件取消不声称 Cookie 已同步。
pub(super) fn record(session: &PublicSession) {
    #[cfg(target_arch = "wasm32")]
    {
        let now = js_sys::Date::now();
        SYNCED_COOKIE.with_borrow_mut(|synced| {
            *synced = now.is_finite().then(|| SyncedCookie {
                user_id: session.user_id.clone(),
                expires_at: session.expires_at.clone(),
                absolute_expires_at: session.absolute_expires_at.clone(),
                synchronized_at: now,
            });
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = session;
}

/// 写入尝试先撤掉提示；未知结果后由新的权威读取或原命令重放同步真实期限。
pub(super) fn invalidate() {
    #[cfg(target_arch = "wasm32")]
    SYNCED_COOKIE.with_borrow_mut(|synced| *synced = None);
}
