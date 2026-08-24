//! Admin Dioxus surface 的模块边界。
//!
//! `surface.rs` 拥有路由、页面壳和跨页面展示辅助；拍卖、身份、履约、运营和规则页面按
//! 领域子目录分组，API 请求仍由同级 `api/` owner 提供。

mod surface;

pub use surface::run;
