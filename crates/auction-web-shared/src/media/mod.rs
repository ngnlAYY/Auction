//! 浏览器可见视口投影；断点切换只由 CSS `@media` 承担，不经 WASM 往返。

mod viewport;
pub(crate) use viewport::use_visual_viewport;
