//! 静态挂载的核心安全测试：两条 CSP 互不串、指针与清单失败关闭、路径不出挂载根、
//! 缓存头与验证器随策略绑定。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::Response;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::WebMountError;
use crate::assets::SurfacePages;
use crate::assets::tests::TestAssets;
use crate::server::build_identity::build_id;
use crate::{PublicApiState, WebScriptPolicy, public_router_with_pages};

const LEGACY: &str = "legacy-1";
const SOLID: &str = "solid-2";
const LEGACY_INDEX: &[u8] = b"<html>legacy shell</html>";
const SOLID_INDEX: &[u8] = b"<html>solid page</html>";
const DIOXUS_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-eval' \
                          'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                          object-src 'none'; base-uri 'self'; form-action 'self'; \
                          frame-ancestors 'none'";
const SOLID_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' \
                         'unsafe-inline'; object-src 'none'; base-uri 'self'; \
                         form-action 'self'; frame-ancestors 'none'";

fn hex_of(data: &[u8]) -> String {
    auction_application::core::lower_hex(&Sha256::digest(data))
}

/// 测试专用临时目录；析构时清理。
struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "auction-mount-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("创建临时目录");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

type Files = Vec<(&'static str, &'static [u8])>;

fn legacy_files() -> Files {
    vec![
        ("index.html", LEGACY_INDEX),
        ("assets/app-dxh01234567.js", b"legacy-js"),
        ("assets/app-dxh01234567.js.br", b"legacy-js-br"),
        ("assets/app_bg-dxh2.wasm", b"\0asm"),
    ]
}

fn solid_files() -> Files {
    vec![("index.html", SOLID_INDEX), ("assets/index-B2UikXgQ.js", b"solid-js")]
}

/// 一个挂载根：`public/ENTRY.v1` 与两个 build 目录。
struct Mount {
    root: TempRoot,
}

struct Pointer {
    build_id: &'static str,
    stack: &'static str,
    script_policy: &'static str,
    entry_sha256: String,
    payload_sha256: String,
    independent_build_id: &'static str,
    independent_payload_sha256: String,
}

impl Mount {
    fn surface(&self) -> PathBuf {
        self.root.path().join("public")
    }

    /// 写一个 build 目录并返回它的 payload_sha256（测试里取构建号的摘要即可）。
    fn write_build(&self, id: &str, stack: &str, files: &Files) -> String {
        let payload = hex_of(id.as_bytes());
        let build = self.surface().join(id);
        let mut manifest = format!(
            "schema=auction-web-build/1\nsurface=public\nbuild_id={id}\nstack={stack}\npayload_sha256={payload}\n\n"
        );
        for (path, data) in files {
            let target = build.join("files").join(path);
            fs::create_dir_all(target.parent().expect("父目录")).expect("建目录");
            fs::write(&target, data).expect("写文件");
            let kind = if path.ends_with(".br") { "br-entity" } else { "identity" };
            manifest.push_str(&format!("{}  ./{path}  {kind}\n", hex_of(data)));
        }
        fs::write(build.join("MANIFEST.v1"), manifest).expect("写清单");
        payload
    }

    fn write_pointer(&self, pointer: &Pointer) {
        let text = format!(
            "schema=auction-web-entry/1\nsurface=public\nbuild_id={}\nstack={}\n\
             script_policy={}\nentry_sha256={}\npayload_sha256={}\n\
             independent_build_id={}\nindependent_payload_sha256={}\n",
            pointer.build_id,
            pointer.stack,
            pointer.script_policy,
            pointer.entry_sha256,
            pointer.payload_sha256,
            pointer.independent_build_id,
            pointer.independent_payload_sha256,
        );
        fs::write(self.surface().join("ENTRY.v1"), text).expect("写指针");
    }

    fn load(&self) -> Result<SurfacePages, WebMountError> {
        SurfacePages::with_public_mount::<TestAssets>(WebScriptPolicy::Solid, self.root.path())
    }
}

/// 旧壳为入口、新版 Solid 为独立入口的标准挂载。
fn legacy_pointer_mount() -> (Mount, Pointer) {
    legacy_pointer_mount_with(&legacy_files(), &solid_files())
}

fn legacy_pointer_mount_with(legacy: &Files, solid: &Files) -> (Mount, Pointer) {
    let mount = Mount { root: TempRoot::new() };
    fs::create_dir_all(mount.surface()).expect("建 surface 目录");
    let legacy_payload = mount.write_build(LEGACY, "dioxus", legacy);
    let solid_payload = mount.write_build(SOLID, "solid", solid);
    let pointer = Pointer {
        build_id: LEGACY,
        stack: "dioxus",
        script_policy: "dioxus",
        entry_sha256: hex_of(LEGACY_INDEX),
        payload_sha256: legacy_payload,
        independent_build_id: SOLID,
        independent_payload_sha256: solid_payload,
    };
    mount.write_pointer(&pointer);
    (mount, pointer)
}

fn app(pages: SurfacePages) -> Router {
    let state = PublicApiState::default();
    let boundary = state.http_boundary();
    let gate = state.anonymous_read_gate();
    public_router_with_pages(env!("CARGO_PKG_VERSION"), boundary, gate, Arc::new(pages))
        .with_state(state)
}

async fn get(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Response {
    let mut builder = Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    app.clone().oneshot(builder.body(Body::empty()).expect("请求")).await.expect("响应")
}

fn header_text<'a>(response: &'a Response, name: &str) -> &'a str {
    response.headers().get(name).and_then(|value| value.to_str().ok()).unwrap_or_default()
}

async fn body(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 1 << 20).await.expect("正文").to_vec()
}

/// 保护的不变量：旧壳 HTML 只配 Dioxus CSP，新版 Solid HTML 只配 Solid CSP，API 与错误页
/// 永远使用 surface 默认（Solid），旧壳的放宽不渗入非页面响应。
#[tokio::test]
async fn legacy_and_solid_pages_never_swap_their_csp() {
    let (mount, _) = legacy_pointer_mount();
    let app = app(mount.load().expect("标准挂载应加载成功"));

    let legacy = get(&app, "/", &[]).await;
    assert_eq!(legacy.status(), StatusCode::OK);
    assert_eq!(header_text(&legacy, "content-security-policy"), DIOXUS_CSP);
    assert_eq!(body(legacy).await, LEGACY_INDEX);

    for path in ["/consignment", "/consignment/me", "/consignment/new"] {
        let solid = get(&app, path, &[]).await;
        assert_eq!(solid.status(), StatusCode::OK, "{path}");
        assert_eq!(header_text(&solid, "content-security-policy"), SOLID_CSP, "{path}");
        assert_eq!(body(solid).await, SOLID_INDEX, "{path}");
    }

    // 资源按所属 build 配策略；同时 API 404 与错误页不因旧壳放宽。
    let legacy_js = get(&app, "/assets/app-dxh01234567.js", &[]).await;
    assert_eq!(header_text(&legacy_js, "content-security-policy"), DIOXUS_CSP);
    let solid_js = get(&app, "/assets/index-B2UikXgQ.js", &[]).await;
    assert_eq!(header_text(&solid_js, "content-security-policy"), SOLID_CSP);
    for path in ["/api/v1/no-such-route", "/no-such-page"] {
        let other = get(&app, path, &[("accept", "text/html")]).await;
        assert_eq!(other.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(header_text(&other, "content-security-policy"), SOLID_CSP, "{path}");
    }

    // 回切期间：旧壳构建号写入 x-auction-build，独立入口构建号写入另一个头；两者同时出现。
    let api = get(&app, "/api/v1/no-such-route", &[]).await;
    assert_eq!(header_text(&api, "x-auction-build"), LEGACY);
    assert_eq!(header_text(&api, "x-auction-build-independent"), SOLID);
}

/// 反例：脚本策略不能由输入拼出错配——指针冗余字段、清单与独立入口的栈任一不符都拒绝。
#[test]
fn mismatched_stack_and_policy_cannot_be_constructed() {
    let (mount, mut pointer) = legacy_pointer_mount();
    pointer.script_policy = "solid";
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::PolicyMismatch(_))));

    // 指针说 solid，但目录里的清单是 dioxus。
    pointer.script_policy = "solid";
    pointer.stack = "solid";
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::PolicyMismatch(_))));

    // 独立入口必须是 Solid：把它指向旧壳 build。
    let (mount, mut pointer) = legacy_pointer_mount();
    pointer.independent_build_id = LEGACY;
    pointer.independent_payload_sha256 = pointer.payload_sha256.clone();
    pointer.build_id = SOLID;
    pointer.stack = "solid";
    pointer.script_policy = "solid";
    pointer.payload_sha256 = hex_of(SOLID.as_bytes());
    pointer.entry_sha256 = hex_of(SOLID_INDEX);
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::PolicyMismatch(_))));
}

/// 指针引用的文件缺字节或摘要不符、有清单外文件、符号链接、同 URL 异内容：全部启动失败，
/// 不降级回内嵌页面；对照组完整挂载则成功。
#[test]
fn build_ids_that_are_not_a_single_directory_name_are_rejected() {
    let (mount, _) = legacy_pointer_mount();
    assert!(mount.load().is_ok(), "对照组必须通过，否则下面的失败不说明问题");
    for bad in [".", ".."] {
        let (mount, mut pointer) = legacy_pointer_mount();
        pointer.build_id = bad;
        mount.write_pointer(&pointer);
        assert!(mount.load().is_err(), "build_id={bad}");
        let (mount, mut pointer) = legacy_pointer_mount();
        pointer.independent_build_id = bad;
        mount.write_pointer(&pointer);
        assert!(mount.load().is_err(), "independent_build_id={bad}");
    }
}

#[test]
fn incomplete_or_tampered_mounts_fail_closed() {
    let (mount, _) = legacy_pointer_mount();
    assert!(mount.load().is_ok(), "对照组必须通过，否则下面的失败不说明问题");

    // 缺文件。
    let (mount, _) = legacy_pointer_mount();
    fs::remove_file(mount.surface().join(LEGACY).join("files/assets/app_bg-dxh2.wasm"))
        .expect("删文件");
    assert!(matches!(mount.load(), Err(WebMountError::FileMissing(_))));

    // 摘要不符（同长度改一个字节）。
    let (mount, _) = legacy_pointer_mount();
    fs::write(mount.surface().join(LEGACY).join("files/assets/app-dxh01234567.js"), b"legacy-jS")
        .expect("改文件");
    assert!(matches!(mount.load(), Err(WebMountError::DigestMismatch(_))));

    // 清单外多出的文件。
    let (mount, _) = legacy_pointer_mount();
    fs::write(mount.surface().join(SOLID).join("files/assets/extra.js"), b"x").expect("加文件");
    assert!(matches!(mount.load(), Err(WebMountError::FileUnlisted(_))));

    // 入口 HTML 与指针登记的 entry_sha256 不符。
    let (mount, mut pointer) = legacy_pointer_mount();
    pointer.entry_sha256 = hex_of(b"other");
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::DigestMismatch(_))));

    // 清单 payload_sha256 与指针不符。
    let (mount, mut pointer) = legacy_pointer_mount();
    pointer.payload_sha256 = hex_of(b"other");
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::DigestMismatch(_))));

    // 符号链接即使指向字节相同的文件也拒绝。
    let (mount, _) = legacy_pointer_mount();
    let target = mount.surface().join(SOLID).join("files/assets/index-B2UikXgQ.js");
    fs::remove_file(&target).expect("删文件");
    let outside = mount.root.path().join("outside.js");
    fs::write(&outside, b"solid-js").expect("写外部文件");
    std::os::unix::fs::symlink(&outside, &target).expect("建符号链接");
    assert!(matches!(mount.load(), Err(WebMountError::UnsafePath(_))));

    // 同一 URL 在两个 build 里字节不同。
    let mut solid = solid_files();
    solid.push(("assets/app_bg-dxh2.wasm", b"different"));
    let (mount, _) = legacy_pointer_mount_with(&legacy_files(), &solid);
    assert!(matches!(mount.load(), Err(WebMountError::Conflict(_))));

    // 没有入口指针：不能当作“未挂载”静默放行。
    let (mount, _) = legacy_pointer_mount();
    fs::remove_file(mount.surface().join("ENTRY.v1")).expect("删指针");
    assert!(matches!(mount.load(), Err(WebMountError::Io { .. })));
}

/// 清单里的穿越路径在加载期就被拒绝；请求期的穿越与挂载根外的文件都取不到。
#[tokio::test]
async fn paths_cannot_escape_the_mount_root() {
    for bad in ["../outside.js", "assets/../../outside.js", "/etc/passwd", "a//b.js", "a\\b.js"] {
        let (mount, pointer) = legacy_pointer_mount();
        let manifest = mount.surface().join(SOLID).join("MANIFEST.v1");
        let mut text = fs::read_to_string(&manifest).expect("读清单");
        text.push_str(&format!("{}  ./{bad}  identity\n", hex_of(b"x")));
        fs::write(&manifest, text).expect("改清单");
        assert!(
            matches!(mount.load(), Err(WebMountError::UnsafePath(_))),
            "{bad} 必须在加载期被拒绝"
        );
        drop(pointer);
    }

    let (mount, _) = legacy_pointer_mount();
    fs::write(mount.root.path().join("secret.txt"), b"TOP-SECRET").expect("写外部文件");
    let app = app(mount.load().expect("加载"));
    for uri in [
        "/../secret.txt",
        "/assets/../../secret.txt",
        "/assets/%2e%2e/%2e%2e/secret.txt",
        "/assets/..%2fsecret.txt",
        "/ENTRY.v1",
        "/MANIFEST.v1",
        "/files/index.html",
        "/public/ENTRY.v1",
    ] {
        let response = get(&app, uri, &[]).await;
        assert_ne!(response.status(), StatusCode::OK, "{uri}");
        let bytes = body(response).await;
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("TOP-SECRET") && !text.contains("schema="), "{uri}");
    }
}

/// 缓存与验证器：指纹资源按所属 build 的栈判定不可变；挂载时 HTML 的 ETag 混入策略标识，
/// 同一份 HTML 在不同策略下不会被 304 复用；纯内嵌保持历史 ETag。
#[tokio::test]
async fn cache_headers_and_validators_follow_the_page_policy() {
    // 两个 build 的入口 HTML 字节完全相同，只有策略不同。
    let same_html: &[u8] = b"<html>same bytes</html>";
    let mut legacy = legacy_files();
    legacy[0] = ("index.html", same_html);
    let mut solid = solid_files();
    solid[0] = ("index.html", same_html);
    let (mount, mut pointer) = legacy_pointer_mount_with(&legacy, &solid);
    pointer.entry_sha256 = hex_of(same_html);
    mount.write_pointer(&pointer);
    let app = app(mount.load().expect("加载"));

    let legacy_page = get(&app, "/", &[]).await;
    let solid_page = get(&app, "/consignment", &[]).await;
    assert_eq!(header_text(&legacy_page, "cache-control"), "no-cache");
    assert_eq!(header_text(&legacy_page, "vary"), "Accept-Encoding");
    let legacy_etag = header_text(&legacy_page, "etag").to_owned();
    let solid_etag = header_text(&solid_page, "etag").to_owned();
    assert_ne!(legacy_etag, solid_etag, "同一 HTML、不同策略的 ETag 必须不同");
    assert_ne!(legacy_etag, format!("\"{}\"", hex_of(same_html)), "挂载时 ETag 混入策略标识");

    // 正例：同策略同 URL 的条件请求命中 304；反例：拿旧策略的 ETag 问新策略的页面不能 304。
    let revalidated = get(&app, "/", &[("if-none-match", &legacy_etag)]).await;
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_text(&revalidated, "content-security-policy"), DIOXUS_CSP);
    let crossed = get(&app, "/consignment", &[("if-none-match", &legacy_etag)]).await;
    assert_eq!(crossed.status(), StatusCode::OK);
    assert_eq!(header_text(&crossed, "content-security-policy"), SOLID_CSP);

    // 旧壳的 dxh 指纹资源在 Dioxus build 内不可变；换成 Solid 命名规则则不是。
    let dxh = get(&app, "/assets/app-dxh01234567.js", &[]).await;
    assert_eq!(header_text(&dxh, "cache-control"), "public, max-age=31536000, immutable");
    let solid_asset = get(&app, "/assets/index-B2UikXgQ.js", &[]).await;
    assert_eq!(header_text(&solid_asset, "cache-control"), "public, max-age=31536000, immutable");
    let wasm = get(&app, "/assets/app_bg-dxh2.wasm", &[]).await;
    assert_eq!(header_text(&wasm, "cache-control"), "no-cache");

    // 预压缩实体：按 br 协商原样发出；原文被拒且没有 br 副本时 406 而不是降级。
    let brotli = get(&app, "/assets/app-dxh01234567.js", &[("accept-encoding", "br")]).await;
    assert_eq!(header_text(&brotli, "content-encoding"), "br");
    assert_eq!(body(brotli).await, b"legacy-js-br");
    let refused = get(&app, "/", &[("accept-encoding", "identity;q=0")]).await;
    assert_eq!(refused.status(), StatusCode::NOT_ACCEPTABLE);

    // 纯内嵌（没有挂载配置）仍是历史的内容摘要 ETag。
    let embedded = app_embedded();
    let page = get(&embedded, "/", &[]).await;
    let embedded_index = include_bytes!("../../../tests/assets-fixture/index.html");
    assert_eq!(header_text(&page, "etag"), format!("\"{}\"", hex_of(embedded_index)));
}

fn app_embedded() -> Router {
    app(SurfacePages::embedded::<TestAssets>(WebScriptPolicy::Solid))
}

/// 正常 Solid 运行：指针指向本二进制的内嵌 build 时不依赖任何旧壳目录，两个构建号相同。
#[tokio::test]
async fn pointer_to_the_embedded_build_needs_no_legacy_directory() {
    let embedded_index = include_bytes!("../../../tests/assets-fixture/index.html");
    let mount = Mount { root: TempRoot::new() };
    fs::create_dir_all(mount.surface()).expect("建 surface 目录");
    let mut pointer = Pointer {
        build_id: build_id_static(),
        stack: "solid",
        script_policy: "solid",
        entry_sha256: hex_of(embedded_index),
        payload_sha256: hex_of(b"p"),
        independent_build_id: build_id_static(),
        independent_payload_sha256: hex_of(b"p"),
    };
    mount.write_pointer(&pointer);
    let app = app(mount.load().expect("内嵌入口应加载成功"));
    for path in ["/", "/consignment"] {
        let response = get(&app, path, &[]).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(header_text(&response, "content-security-policy"), SOLID_CSP);
    }
    let api = get(&app, "/api/v1/no-such-route", &[]).await;
    assert_eq!(header_text(&api, "x-auction-build"), build_id());
    assert_eq!(header_text(&api, "x-auction-build-independent"), build_id());

    // 反例：内嵌 HTML 摘要与指针不符 → 失败关闭。
    pointer.entry_sha256 = hex_of(b"other");
    mount.write_pointer(&pointer);
    assert!(matches!(mount.load(), Err(WebMountError::DigestMismatch(_))));
}

fn build_id_static() -> &'static str {
    build_id()
}
