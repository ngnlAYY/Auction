//! 每表独立的非敏感展示偏好；失败存储不影响当前页，不读取 Public 偏好。

pub(super) fn parse(value: &str, maximum: Option<i64>) -> Option<i64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<i64>().ok())
        .flatten()
        .filter(|size| *size > 0 && maximum.is_none_or(|maximum| *size <= maximum))
}

pub(super) fn load(scope: &str, maximum: Option<i64>) -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        auction_web_shared::storage::get_item(&format!("hv.admin.page-size:{scope}"))
            .as_deref()
            .and_then(|value| parse(value, maximum))
            .unwrap_or(10)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (scope, maximum);
        10
    }
}

pub(super) fn save(scope: &str, size: i64) {
    #[cfg(target_arch = "wasm32")]
    let _ = auction_web_shared::storage::set_item(
        &format!("hv.admin.page-size:{scope}"),
        &size.to_string(),
    );
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (scope, size);
}
