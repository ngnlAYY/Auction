//! Public 每页数量的本地偏好；只存展示数量，URL 明确值优先且不会因访问链接覆盖偏好。

use super::{DEFAULT_PAGE_SIZE, MAX_PUBLIC_PAGE_SIZE};

#[cfg(target_arch = "wasm32")]
const PAGE_SIZE_KEY: &str = "hv.public.page-size";
#[cfg(target_arch = "wasm32")]
const MOVEMENT_PAGE_SIZE_KEY: &str = "hv.public.movement-page-size";

/// URL 与本地存储使用同一严格解析，不接受符号、小数、科学计数法、空白或越界值。
#[cfg(target_arch = "wasm32")]
pub(crate) fn parse_public_page_size(value: &str) -> Option<i64> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse::<i64>().ok())
        .flatten()
        .filter(|size| (1..=MAX_PUBLIC_PAGE_SIZE).contains(size))
}

/// 所有主列表共享偏好，补偿流水独立保存；损坏或受限存储回退默认值而非阻止页面加载。
pub(crate) fn stored_public_page_size(movement: bool) -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        let key = if movement { MOVEMENT_PAGE_SIZE_KEY } else { PAGE_SIZE_KEY };
        return web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .and_then(|storage| storage.get_item(key).ok().flatten())
            .and_then(|value| parse_public_page_size(&value))
            .unwrap_or(DEFAULT_PAGE_SIZE);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = movement;
        DEFAULT_PAGE_SIZE
    }
}

/// 只在用户主动确认合法数量后保存；写入失败不影响本次分页和 URL。
pub(super) fn save_public_page_size(size: i64, movement: bool) {
    if !(1..=MAX_PUBLIC_PAGE_SIZE).contains(&size) {
        return;
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let key = if movement { MOVEMENT_PAGE_SIZE_KEY } else { PAGE_SIZE_KEY };
        let _ = storage.set_item(key, &size.to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = movement;
}
