//! Admin 部署制品独占的浏览器静态资源类型；`web-solid` 选 Solid 产物，
//! 其余显式 `web-embed` 的构建选 Dioxus 产物，缺失产物由 HTTP 路由返回 503。
//! API-only 构建使用固定缺失目录，避免机器残留的 Web 产物被烘入二进制。
//! 配置了 `[web].mount_root` 时，正式 Admin 页面改由启动时加载并核对过的静态挂载提供；首次设置页始终内嵌。

use auction_config::AppConfig;
use auction_http::{SurfacePages, WebMountError, WebScriptPolicy};
use rust_embed::Embed;

/// 启用 `web-solid` 时读取由 `web-bundle.sh admin` / `bun run build:admin` 写入的 Admin 产物目录。
#[cfg(feature = "web-solid")]
#[derive(Embed)]
#[folder = "../../target/web/admin/public"]
#[allow_missing = true]
pub struct AdminWebAssets;

/// `dx bundle` 生成并复制到稳定嵌入目录的 Admin 浏览器产物。
#[cfg(all(feature = "web-embed", not(feature = "web-solid")))]
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

/// 由本 surface 内嵌产物对应的 feature 得出的浏览器脚本策略。
///
/// `web-solid` 蕴含 `web-embed`；只提供 API 的构建没有页面，取两者中较严的 Solid，
/// 不为不存在的 WASM 放开 eval。资源类型与策略共用同一组 cfg，调用方不再各自推断。
pub const fn web_script_policy() -> WebScriptPolicy {
    if cfg!(feature = "web-solid") {
        WebScriptPolicy::Solid
    } else if cfg!(feature = "web-embed") {
        WebScriptPolicy::Dioxus
    } else {
        WebScriptPolicy::Solid
    }
}

/// 按 `[web].mount_root` 构造正式 Admin 页面：未配置时纯内嵌，响应与未支持挂载的程序逐字节一致；
/// 配置时读取并完整核对 `<mount_root>/admin`，任何不一致返回错误，调用方据此终止启动（E1）。
pub(crate) fn surface_pages(config: &AppConfig) -> Result<SurfacePages, WebMountError> {
    match config.web.mount_root() {
        None => Ok(SurfacePages::embedded::<AdminWebAssets>(web_script_policy())),
        Some(root) => SurfacePages::with_admin_mount::<AdminWebAssets>(web_script_policy(), root),
    }
}

#[cfg(test)]
mod tests {
    use auction_http::WebScriptPolicy;

    use super::web_script_policy;

    /// 两套产物目录即使都在磁盘上，只提供 API 的构建也只能看到固定的空目录。
    #[cfg(not(feature = "web-embed"))]
    #[test]
    fn api_only_build_embeds_no_surface_assets() {
        assert_eq!(super::AdminWebAssets::iter().count(), 0);
    }

    #[test]
    fn script_policy_follows_the_embedded_stack() {
        #[cfg(feature = "web-solid")]
        let expected = WebScriptPolicy::Solid;
        #[cfg(all(feature = "web-embed", not(feature = "web-solid")))]
        let expected = WebScriptPolicy::Dioxus;
        #[cfg(not(feature = "web-embed"))]
        let expected = WebScriptPolicy::Solid;
        assert_eq!(web_script_policy(), expected);
    }
}
