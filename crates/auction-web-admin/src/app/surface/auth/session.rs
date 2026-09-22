use super::*;

#[cfg(target_arch = "wasm32")]
const SESSION_REFRESH_SAFETY_MARGIN_MS: f64 = 1_000.0;
#[cfg(target_arch = "wasm32")]
const MIN_SESSION_REFRESH_DELAY_MS: f64 = 1_000.0;

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

/// 每次会话读取成功后安排下一次 GET；即使绝对过期上限使 deadline 不变，也会由父组件
/// 的已完成检查代数重挂载，在截止点之后读到服务端权威的未认证结果。
#[component]
pub(crate) fn AdminSessionRefresh(
    session: AdminSession,
    mut reload_generation: Signal<u64>,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    {
        let delay = session_refresh_delay(&session.expires_at, &session.absolute_expires_at);
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
