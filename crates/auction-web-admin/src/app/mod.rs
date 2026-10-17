//! Admin Dioxus surface 的模块边界。
//! `surface/mod.rs` 拥有 surface 装配、资源初始化和跨页面展示辅助；路由与页面入口位于
//! `surface/router.rs`；拍卖、身份、履约、运营和规则页面按
//! 领域子目录分组，API 请求仍由同级 `api/` owner 提供。

mod surface;

pub use surface::run;
