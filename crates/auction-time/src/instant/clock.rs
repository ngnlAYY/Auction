//! 系统时钟读取：原生目标用操作系统时钟，浏览器目标用 `Date.now()`。
//!
//! wasm32 的标准库没有时钟，调用即崩溃。浏览器分支直接把毫秒数换算成时刻，不经时间库的
//! 系统时间转换路径，后者会把整套错误格式化代码编进 wasm。

use super::Timestamp;

/// 当前时刻；浏览器里精度为毫秒，时钟读数不可用时退回 Unix 纪元。
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(super) fn system_now() -> Timestamp {
    let milliseconds = js_sys::Date::now();
    // 先确认读数落在 `i64` 之内再转换；浏览器时钟不会给出这之外的值。
    let in_range = milliseconds.is_finite()
        && milliseconds >= i64::MIN as f64
        && milliseconds <= i64::MAX as f64;
    in_range
        .then(|| Timestamp::from_timestamp_millis(milliseconds as i64))
        .flatten()
        .unwrap_or(Timestamp::UNIX_EPOCH)
}

/// 当前时刻。
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(super) fn system_now() -> Timestamp {
    Timestamp(jiff::Timestamp::now())
}
