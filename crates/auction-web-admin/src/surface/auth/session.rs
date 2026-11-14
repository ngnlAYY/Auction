//! Admin 会话的重试与到期前续期。

#[cfg(target_arch = "wasm32")]
use auction_web_shared::timer::TimeoutFuture;
use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
const SESSION_REFRESH_SAFETY_MARGIN_MS: f64 = 1_000.0;
#[cfg(target_arch = "wasm32")]
const MIN_SESSION_REFRESH_DELAY_MS: f64 = 1_000.0;

/// 会话尚未确认时每 30 秒重试；重新确认后卸载，不替换已认证页面子树。
#[component]
pub(crate) fn AdminSessionRetry(mut reload_generation: Signal<u64>) -> Element {
    #[cfg(target_arch = "wasm32")]
    use_future(move || async move {
        loop {
            TimeoutFuture::new(30_000).await;
            reload_generation.set(reload_generation().saturating_add(1));
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    let _ = reload_generation;
    rsx! {}
}

/// 每次检查完成后重设下一次 GET；即使绝对截止未变，也必须在截止后再次确认身份。
/// 定时器只持有截止字段，不复制 actor 或 CSRF token。
#[component]
pub(crate) fn AdminSessionRefresh(
    checked_generation: u64,
    expires_at: String,
    absolute_expires_at: String,
    mut reload_generation: Signal<u64>,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    use_resource(use_reactive(
        (&checked_generation, &expires_at, &absolute_expires_at),
        move |(_, expires_at, absolute_expires_at)| async move {
            let delay = session_refresh_delay(&expires_at, &absolute_expires_at);
            TimeoutFuture::new(delay).await;
            reload_generation.set(reload_generation().saturating_add(1));
        },
    ));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (checked_generation, expires_at, absolute_expires_at, reload_generation);
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn session_refresh_delay(expires_at: &str, absolute_expires_at: &str) -> u32 {
    let Ok(expires_at) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return 30_000;
    };
    let Ok(absolute_expires_at) = chrono::DateTime::parse_from_rfc3339(absolute_expires_at) else {
        return 30_000;
    };
    let deadline = expires_at.timestamp_millis().min(absolute_expires_at.timestamp_millis()) as f64;
    let delay = deadline - js_sys::Date::now() - SESSION_REFRESH_SAFETY_MARGIN_MS;
    if !delay.is_finite() {
        return 30_000;
    }
    delay.clamp(MIN_SESSION_REFRESH_DELAY_MS, 2_147_483_647.0) as u32
}
