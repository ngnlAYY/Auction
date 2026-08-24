use super::*;

/// 会话查询失败时按旧版策略每 30 秒重试；成功后该组件随错误页一起卸载。
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

/// 在 Admin 会话滑动过期前后触发一次 GET，复用服务端的 touch 语义刷新 Cookie 会话。
#[component]
pub(crate) fn AdminSessionRefresh(
    session: AdminSession,
    mut reload_generation: Signal<u64>,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let delay = session_refresh_delay(&session.expires_at);
        use_future(move || async move {
            TimeoutFuture::new(delay).await;
            reload_generation.set(reload_generation().saturating_add(1));
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (session, reload_generation);
    }
    rsx! {}
}

#[cfg(target_arch = "wasm32")]
fn session_refresh_delay(expires_at: &str) -> u32 {
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return 30_000;
    };
    let delay = expiry.timestamp_millis() as f64 - js_sys::Date::now();
    if !delay.is_finite() {
        return 30_000;
    }
    delay.clamp(0.0, 2_147_483_647.0) as u32
}
