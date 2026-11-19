//! 无参数译文的共享实现。

use super::{translation_failed, without_fluent_isolation};

/// 读取当前 Dioxus 作用域的译文，集中处理结果以减少 WASM 中的重复展开。
#[doc(hidden)]
#[inline(never)]
pub fn translate_without_args(id: &str) -> String {
    match dioxus_i18n::te!(id) {
        Ok(translated) => without_fluent_isolation(translated),
        Err(error) => translation_failed(id, &error),
    }
}
