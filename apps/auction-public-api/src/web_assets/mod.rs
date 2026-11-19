//! Public 部署制品独占的浏览器静态资源类型；`web-solid` 选 Solid 产物，
//! 其余显式 `web-embed` 的构建选 Dioxus 产物，缺失产物由 HTTP 路由返回 503。
//! API-only 构建使用固定缺失目录，避免机器残留的 Web 产物被烘入二进制。

use auction_http::WebScriptPolicy;
use rust_embed::Embed;

/// `mise run web-build` 在 `AUCTION_WEB_STACK=solid` 下生成并复制到稳定目录的 Public 产物。
#[cfg(feature = "web-solid")]
#[derive(Embed)]
#[folder = "../../target/web/public/public"]
#[allow_missing = true]
pub(crate) struct PublicWebAssets;

/// `dx bundle` 生成并复制到稳定嵌入目录的 Public 浏览器产物。
#[cfg(all(feature = "web-embed", not(feature = "web-solid")))]
#[derive(Embed)]
#[folder = "../../target/dx/build/auction-web-public/web/public"]
#[allow_missing = true]
pub(crate) struct PublicWebAssets;

/// API-only 制品使用的确定性空资源集合。
#[cfg(not(feature = "web-embed"))]
#[derive(Embed)]
#[folder = "../../target/dist/api-only-assets/public"]
#[allow_missing = true]
pub(crate) struct PublicWebAssets;

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

#[cfg(test)]
mod tests {
    use auction_http::WebScriptPolicy;

    use super::web_script_policy;

    /// 两套产物目录即使都在磁盘上，只提供 API 的构建也只能看到固定的空目录。
    #[cfg(not(feature = "web-embed"))]
    #[test]
    fn api_only_build_embeds_no_surface_assets() {
        assert_eq!(super::PublicWebAssets::iter().count(), 0);
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
