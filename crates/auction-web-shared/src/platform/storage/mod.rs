//! 浏览器 `localStorage` 的容错读写与脚本可见 Cookie 的只读访问。
//! 存储不可用、被隐私设置禁用、配额不足或值损坏都只返回缺失/失败，不 panic、不影响当前页面；
//! 非 WASM 构建（本地静态检查与服务端测试）恒为空操作，调用方无需为此分支。
//! 本模块不持有键名，偏好与缓存的键由各 surface 自行声明。

use serde::de::DeserializeOwned;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

/// 单条本地 JSON 值的解析上限：拒绝被其他脚本写入或损坏的超大值，而不是部分解析。
const MAX_PARSED_BYTES: usize = 2_000_000;

/// 读取指定的脚本可见 Cookie。外层为空时无法确认，内层为空时已确认该键缺失。
pub fn read_cookie(name: &str) -> Option<Option<String>> {
    #[cfg(target_arch = "wasm32")]
    {
        let document = web_sys::window()?.document()?;
        let cookies =
            js_sys::Reflect::get(&document, &JsValue::from_str("cookie")).ok()?.as_string()?;
        cookie_value(&cookies, name)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = name;
        None
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn cookie_value(cookies: &str, name: &str) -> Option<Option<String>> {
    let mut values = cookies
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(key, _)| *key == name)
        .map(|(_, value)| value);
    let value = values.next();
    // 同名 Cookie 会让请求使用的代际不明确，不能据此确认会话结束。
    if values.next().is_some() {
        return None;
    }
    Some(value.filter(|value| !value.is_empty()).map(str::to_owned))
}

/// 读取原始字符串值；缺失或读取失败返回 `None`。
pub fn get_item(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?.local_storage().ok()??.get_item(key).ok()?
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

/// 写入字符串值；成功返回 `true`。配额不足等失败只返回 `false`，不删除或覆盖其他键。
#[must_use]
pub fn set_item(key: &str, value: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .is_some_and(|storage| storage.set_item(key, value).is_ok())
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
        false
    }
}

/// 删除单个键；键不存在或存储不可用时静默返回。
pub fn remove_item(key: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.remove_item(key);
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

/// 当前存储中的全部键；按浏览器给出的顺序返回，存储不可用时为空。
pub fn keys() -> Vec<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return Vec::new();
        };
        (0..storage.length().unwrap_or_default())
            .filter_map(|index| storage.key(index).ok().flatten())
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

/// 读取并反序列化 JSON 值；缺失、超过解析上限或格式损坏时返回 `None`。
pub fn get_parsed<T: DeserializeOwned>(key: &str) -> Option<T> {
    let raw = get_item(key)?;
    if raw.len() > MAX_PARSED_BYTES {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::cookie_value;

    #[test]
    fn cookie_read_distinguishes_absence_from_ambiguity() {
        assert_eq!(
            cookie_value("example=other; test=placeholder", "test"),
            Some(Some("placeholder".to_owned()))
        );
        assert_eq!(cookie_value("test_extra=placeholder", "test"), Some(None));
        assert_eq!(cookie_value("test=; test=placeholder", "test"), None);
    }
}
