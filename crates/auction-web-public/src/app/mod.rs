//! Public Dioxus surface 的模块边界。
//! `surface/` 按路由、看板、目录、页面和查询状态拆分跨页面展示职责；拍卖、内容和身份
//! 实现按领域分组在 `surface/` 下，API 请求由 crate 级 `api/` owner 提供。

mod surface;

pub use surface::run;
