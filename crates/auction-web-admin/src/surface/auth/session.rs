//! Admin 会话的重试、到期前续期与写许可判定。

#[cfg(target_arch = "wasm32")]
use auction_web_shared::timer::TimeoutFuture;
use dioxus::prelude::*;

use crate::api::AdminSession;

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
/// 定时器只持有截止字段，不复制 actor 或 CSRF token；它触发的是例行复核，先登记
/// `routine_check` 再递增代数，写许可据此区分例行复核与显式复核。
#[component]
pub(crate) fn AdminSessionRefresh(
    checked_generation: u64,
    expires_at: String,
    absolute_expires_at: String,
    mut reload_generation: Signal<u64>,
    mut routine_check: Signal<Option<u64>>,
) -> Element {
    #[cfg(target_arch = "wasm32")]
    use_resource(use_reactive(
        (&checked_generation, &expires_at, &absolute_expires_at),
        move |(_, expires_at, absolute_expires_at)| async move {
            let delay = session_refresh_delay(&expires_at, &absolute_expires_at);
            TimeoutFuture::new(delay).await;
            // peek：定时器任务不能订阅代数，否则自身递增会重启定时器，在复核往返期间重复触发。
            let next = reload_generation.peek().saturating_add(1);
            routine_check.set(Some(next));
            reload_generation.set(next);
        },
    ));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (checked_generation, expires_at, absolute_expires_at, reload_generation, routine_check);
    rsx! {}
}

/// 写许可判定，保护不变量“会话被确认前不提交写命令”。
///
/// 最近一次完成的检查必须属于当前代数并确认了会话。唯一例外是定时器登记的例行复核在途，
/// 且上一代（紧邻）结果正是同一已确认会话：此时保留写许可，避免每次到期前复核都禁用整页
/// 表单；每次写入仍由 `fetch_csrf_token` 重新 GET 会话并核对 actor 与 CSRF，会话失效即失败
/// 关闭。重试按钮、CSRF 失败触发的复核与登录/退出不登记例行代数，一律走严格分支。
pub(super) fn session_write_permission(
    loaded_generation: u64,
    current_generation: u64,
    loaded: Option<&AdminSession>,
    confirmed: Option<&AdminSession>,
    routine_generation: Option<u64>,
) -> bool {
    let (Some(loaded), Some(confirmed)) = (loaded, confirmed) else {
        return false;
    };
    if loaded_generation == current_generation {
        return true;
    }
    routine_generation == Some(current_generation)
        && loaded_generation.checked_add(1) == Some(current_generation)
        && loaded.actor == confirmed.actor
        && loaded.csrf_token == confirmed.csrf_token
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

#[cfg(test)]
mod tests {
    use super::session_write_permission;
    use crate::api::AdminSession;

    fn session(actor: &str, csrf_token: &str) -> AdminSession {
        AdminSession {
            actor: actor.to_owned(),
            csrf_token: csrf_token.to_owned(),
            expires_at: "2026-11-14T00:15:00Z".to_owned(),
            absolute_expires_at: "2026-11-14T08:00:00Z".to_owned(),
        }
    }

    #[test]
    fn completed_check_for_current_generation_requires_confirmed_session() {
        let current = session("ops", "csrf-a");
        assert!(session_write_permission(4, 4, Some(&current), Some(&current), None));
        assert!(!session_write_permission(4, 4, None, Some(&current), None));
        assert!(!session_write_permission(4, 4, Some(&current), None, None));
    }

    #[test]
    fn routine_recheck_in_flight_keeps_permission_for_same_confirmed_session() {
        let current = session("ops", "csrf-a");
        assert!(session_write_permission(4, 5, Some(&current), Some(&current), Some(5)));
    }

    #[test]
    fn explicit_recheck_in_flight_revokes_permission() {
        let current = session("ops", "csrf-a");
        // 重试按钮、CSRF 失败与登录/退出都不登记例行代数。
        assert!(!session_write_permission(4, 5, Some(&current), Some(&current), None));
        // 例行复核被随后的显式复核取代：标记仍指向旧代数。
        assert!(!session_write_permission(4, 6, Some(&current), Some(&current), Some(5)));
        // 已完成的结果不是紧邻上一代。
        assert!(!session_write_permission(3, 5, Some(&current), Some(&current), Some(5)));
    }

    #[test]
    fn routine_recheck_after_failed_or_changed_session_stays_strict() {
        let current = session("ops", "csrf-a");
        assert!(!session_write_permission(4, 5, None, Some(&current), Some(5)));
        assert!(!session_write_permission(4, 5, Some(&current), None, Some(5)));
        let rotated = session("ops", "csrf-b");
        assert!(!session_write_permission(4, 5, Some(&rotated), Some(&current), Some(5)));
        let other = session("auditor", "csrf-a");
        assert!(!session_write_permission(4, 5, Some(&other), Some(&current), Some(5)));
    }
}
