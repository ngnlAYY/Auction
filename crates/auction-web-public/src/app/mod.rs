//! Public Dioxus surface 的模块边界。
//!
//! `surface.rs` 拥有路由、页面状态和跨页面展示辅助；拍卖、内容、身份和分页实现按职责
//! 放在对应子目录，API 请求仍由同级 `api/` owner 提供。

mod surface;

pub use surface::run;
