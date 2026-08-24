//! Public Dioxus surface 的模块边界。
//!
//! `surface/` 按路由、看板、目录、页面和查询状态拆分跨页面展示职责；拍卖、内容、身份和分页
//! 实现按职责放在对应子目录，API 请求仍由同级 `api/` owner 提供。

mod surface;

pub use surface::run;
