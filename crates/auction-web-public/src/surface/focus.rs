//! 按元素 ID 归还键盘焦点：菜单或面板收起后把焦点还给入口按钮，避免落到 `body`。

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// 把键盘焦点移到指定 ID 的元素；元素不存在或不是 `HtmlElement` 时保持当前焦点。
#[cfg(target_arch = "wasm32")]
pub(crate) fn focus_element_by_id(element_id: &str) {
    if let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(element_id))
        .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
    {
        // 聚焦失败只影响焦点位置，不改变任何状态，无需上报。
        let _ = element.focus();
    }
}

/// 宿主构建没有 DOM，归还焦点是空操作。
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn focus_element_by_id(_element_id: &str) {}
