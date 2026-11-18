//! 浏览器 wire 字段的通用格式谓词；两端 DTO 解码与路由参数共用，不承载业务规则。

use dioxus::html::FormEvent;
use dioxus::prelude::MountedEvent;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// 原生 change 已改变选项。提交被拒时显式恢复 DOM，原 props 不变时虚拟 DOM 不会重写它。
pub fn restore_select_value(event: &FormEvent, applied: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(select) = event
        .data()
        .downcast::<web_sys::Event>()
        .and_then(web_sys::Event::target)
        .and_then(|target| target.dyn_into::<web_sys::HtmlSelectElement>().ok())
    {
        select.set_value(applied);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (event, applied);
}

/// 显式恢复尚未提交的输入值，保留原控件及焦点。
pub(crate) fn restore_input_value(event: &MountedEvent, applied: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(input) = event
        .data()
        .downcast::<web_sys::Element>()
        .and_then(|element| element.dyn_ref::<web_sys::HtmlInputElement>())
    {
        input.set_value(applied);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (event, applied);
}

/// 单行输入会剔除换行，必要时提供可读的转义原文。
pub fn escaped_single_line_input(value: &str) -> Option<String> {
    value.contains(['\r', '\n']).then(|| format!("{value:?}"))
}

/// 旧版浏览器 decoder 接受的 UUID：36 字节小写连字符规范形式、版本 1 至 5、RFC 4122 variant。
/// 逐字节判断，不解析也不分配；nil UUID 的版本为 0，因此同样被拒绝。
pub fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
        && matches!(bytes[14], b'1'..=b'5')
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}
