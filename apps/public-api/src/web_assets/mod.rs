//! Public 部署制品独占的浏览器静态资源类型。
//!
//! 类型位于 Public 装配根，依赖图中不引用 Admin 产物；`allow_missing` 保证首次构建后端时仍能
//! 编译，并由 HTTP 路由在运行时明确返回 503。

use rust_embed::Embed;

/// `dx build` 生成的 Public 浏览器产物。
#[derive(Embed)]
#[folder = "../../target/dx/build/auction-web-public/web/public"]
#[allow_missing = true]
pub(crate) struct PublicWebAssets;
