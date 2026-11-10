//! 浏览器 `localStorage` 的容错读写。
//! 存储不可用、被隐私设置禁用、配额不足或值损坏都只返回缺失/失败，不 panic、不影响当前页面；
//! 非 WASM 构建（本地静态检查与服务端测试）恒为空操作，调用方无需为此分支。
//! 本模块不持有键名，偏好与缓存的键由各 surface 自行声明。

use serde::de::DeserializeOwned;

/// 单条本地 JSON 值的解析上限：拒绝被其他脚本写入或损坏的超大值，而不是部分解析。
const MAX_PARSED_BYTES: usize = 2_000_000;

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
