//! Admin 拍卖页面共享的状态、时间和文案映射。

use auction_web_shared::legacy_t as t;

pub(super) fn admin_auction_status_label(status: &str) -> String {
    match status {
        "draft" => t!("admin-auctions-status-draft"),
        "preview" => t!("admin-auctions-status-preview"),
        "live" => t!("admin-auctions-status-live"),
        "final" => t!("admin-auctions-status-final"),
        "settling" => t!("admin-auctions-status-settling"),
        "settled" => t!("admin-auctions-status-settled"),
        "cancelled" => t!("admin-auctions-status-cancelled"),
        _ => status.to_owned(),
    }
}

pub(super) fn current_time_millis() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::new_0().get_time() as i64
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Utc::now().timestamp_millis()
    }
}

pub(super) fn scheduled_time_millis(value: &str) -> Option<i64> {
    #[cfg(target_arch = "wasm32")]
    {
        let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
        let milliseconds = date.get_time();
        milliseconds.is_finite().then_some(milliseconds as i64)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::DateTime::parse_from_rfc3339(value).ok().map(|value| value.timestamp_millis())
    }
}

pub(super) fn valid_auction_schedule(preview: &str, open: &str, end: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(preview) = local_datetime_millis(preview) else {
            return false;
        };
        let Some(open) = local_datetime_millis(open) else {
            return false;
        };
        let Some(end) = local_datetime_millis(end) else {
            return false;
        };
        preview >= js_sys::Date::new_0().get_time() && open >= preview && end > open
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let Ok(preview) = chrono::NaiveDateTime::parse_from_str(preview, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        let Ok(open) = chrono::NaiveDateTime::parse_from_str(open, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        let Ok(end) = chrono::NaiveDateTime::parse_from_str(end, "%Y-%m-%dT%H:%M") else {
            return false;
        };
        preview >= chrono::Utc::now().naive_utc() && open >= preview && end > open
    }
}

#[cfg(target_arch = "wasm32")]
fn local_datetime_millis(value: &str) -> Option<f64> {
    // `datetime-local` 按浏览器本地时区解释；这里的时间戳必须与提交时的 UTC 转换使用同一
    // 规则，否则用户在非 UTC 时区创建拍卖时，前端校验与服务端看到的时间会产生偏差。
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    let milliseconds = date.get_time();
    milliseconds.is_finite().then_some(milliseconds)
}

pub(super) fn admin_auction_world_label(world: &str) -> String {
    match world {
        "main" => t!("admin-auctions-world-main"),
        "isekai" => t!("admin-auctions-world-isekai"),
        _ => world.to_owned(),
    }
}
