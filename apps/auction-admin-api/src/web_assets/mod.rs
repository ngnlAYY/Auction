//! Admin 部署制品独占的浏览器静态资源类型。
//!
//! 类型位于 Admin 装配根，依赖图中不引用 Public 产物。只有显式启用 `web-embed` 才会选择
//! `dx bundle` 的稳定目录；API-only 构建使用一个固定缺失目录，避免机器上残留的 Web 产物被
//! 偶然烘入二进制。两种声明都保留 `allow_missing`，由 HTTP 路由在没有资源时明确返回 503。

use rust_embed::Embed;

/// `dx bundle` 生成并复制到稳定嵌入目录的 Admin 浏览器产物。
#[cfg(feature = "web-embed")]
#[derive(Embed)]
#[folder = "../../target/dx/build/auction-web-admin/web/public"]
#[allow_missing = true]
pub struct AdminWebAssets;

/// API-only 制品使用的确定性空资源集合。
#[cfg(not(feature = "web-embed"))]
#[derive(Embed)]
#[folder = "../../target/auction-api-only-assets/admin"]
#[allow_missing = true]
pub struct AdminWebAssets;
