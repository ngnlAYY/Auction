//! Admin 部署制品独占的浏览器静态资源类型；只有显式 `web-embed` 才选择稳定产物目录，
//! 缺失产物由 HTTP 路由返回 503。
//! API-only 构建使用固定缺失目录，避免机器残留的 Web 产物被烘入二进制。

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
#[folder = "../../target/dist/api-only-assets/admin"]
#[allow_missing = true]
pub struct AdminWebAssets;
