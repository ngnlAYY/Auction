//! 前端静态产物内嵌与 SPA fallback。
//!
//! Public 与 Admin 各自读取自己的 `dx build` 产物，路由不会跨 surface 提供资源。当前两种
//! `RustEmbed` 类型仍编译在同一个 `auction-http` crate 中，尚未通过 Cargo feature 或独立 crate
//! 建立制品级隔离，因此不能依赖链接器裁剪来宣称二进制一定不包含另一端资源。
//!
//! `allow_missing = true` 是必需的。`rust-embed` 默认在目标目录不存在时**编译期报错**，
//! 而前端产物由独立的 `dx build` 生成；没有这个属性，首次克隆仓库、CI 静态检查或只改后端
//! 的构建都会因为缺少产物而无法编译。缺失产物在运行时以 503 明确报告，不 panic。
//!
//! `folder` 的相对路径由 `rust-embed` 按本 crate 的 `CARGO_MANIFEST_DIR` 解析。
//! release 构建把产物烘进二进制，dev 构建从磁盘读取，因此本地改前端不必重编后端。

use std::borrow::Cow;

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::{Embed, EmbeddedFile};

/// SPA 入口文档。未命中静态文件的浏览器路由都回退到它。
const INDEX_HTML: &str = "index.html";

/// 公开端前端产物。
#[derive(Embed)]
#[folder = "../../target/dx/build/auction-web-public/web/public"]
#[allow_missing = true]
struct PublicWebAssets;

/// 管理端前端产物。
#[derive(Embed)]
#[folder = "../../target/dx/build/auction-web-admin/web/public"]
#[allow_missing = true]
struct AdminWebAssets;

/// 处理公开端的静态产物与浏览器路由。
pub(crate) async fn serve_public_asset(uri: Uri) -> Response {
    serve::<PublicWebAssets>(uri.path())
}

/// 处理管理端的静态产物与浏览器路由。
pub(crate) async fn serve_admin_asset(uri: Uri) -> Response {
    serve::<AdminWebAssets>(uri.path())
}

/// 按请求路径返回内嵌产物。
///
/// 命中顺序：精确匹配静态文件 → 无扩展名路径回退 `index.html` → 产物缺失报 503。
/// 带扩展名且未命中的路径按 404 处理，不回退到 HTML：否则缺失的 `.wasm`/`.js` 会拿到
/// 一份 200 的 HTML，浏览器端只会报难以定位的解析错误。
fn serve<A: rust_embed::RustEmbed>(path: &str) -> Response {
    let relative_path = path.trim_start_matches('/');

    if let Some(file) = A::get(relative_path) {
        return asset_response(relative_path, file);
    }

    if has_file_extension(relative_path) {
        return not_found();
    }

    match A::get(INDEX_HTML) {
        Some(index) => asset_response(INDEX_HTML, index),
        None => assets_missing(),
    }
}

/// 判断路径是否指向具体文件。只看最后一段，避免把 `/lots/one-001` 误判为文件。
fn has_file_extension(relative_path: &str) -> bool {
    relative_path.rsplit('/').next().is_some_and(|segment| segment.contains('.'))
}

fn asset_response(relative_path: &str, file: EmbeddedFile) -> Response {
    // release 构建中数据借用自二进制静态段，这里避免多余复制。
    let body = match file.data {
        Cow::Borrowed(bytes) => Body::from(bytes),
        Cow::Owned(bytes) => Body::from(bytes),
    };

    let content_type = HeaderValue::from_str(file.metadata.mimetype())
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

    // 入口文档必须每次校验：它引用带指纹的 WASM/JS 名称，缓存住会让浏览器加载到已被
    // 替换的旧产物。指纹化产物本身可以长期缓存。
    let cache_control = if relative_path == INDEX_HTML {
        HeaderValue::from_static("no-cache")
    } else {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    };

    ([(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, cache_control)], body)
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// 产物缺失时的响应。
///
/// 说明可执行的原因而不是 panic：请求路径不得因为构建产物缺失而让进程崩溃。
fn assets_missing() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, "web assets are not built; run `just web-build`")
        .into_response()
}
