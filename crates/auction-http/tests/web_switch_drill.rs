//! 冷回切演练的回环夹具与逐响应校验（第 8 期 8C-H，自动部分）。
//!
//! 用两份固定夹具（旧 Dioxus 壳的小型 HTML/JS/WASM 与预压缩字节、新版 Solid 页面）按挂载布局
//! 写出只读 build，再按 D9 第 8 节步骤②–⑤ 的可自动部分走一遍：入口指针换到新版、回切旧壳、
//! 再换到新版；每次切换都重新构造 `SurfacePages` 模拟“切换后重启”（服务只在启动时读一次指针），
//! 并对每个响应逐项核对构建号、入口 HTML 摘要与 `Content-Security-Policy` 字节。
//! 浏览器里的 Wasm 启动、交互与跨壳读取属于人工签收，不在此处。
//!
//! 保护的不变量：旧 HTML 只配 Dioxus 策略，新版页面（含回切期间的 `/consignment` 独立入口）
//! 只配 Solid 策略；条件请求与 CDN 重放不能把一个版本的策略带给另一个版本（领域不变量 6：
//! 错配一律失败关闭，不静默放行）。校验器自身的策略比对被放宽时，三个错配用例必须各自失败。
//! Public 与 Admin 两端各自走同样的重启与逐响应核对循环（Admin 用 `AdminApiState::default()` 的
//! 最小装配，只经 `admin_router_with_pages` 的页面回退路径；业务路由不在本演练范围）。
//! 构建号头只在 no-store 响应上发送：入口响应（no-cache）必须不带，API 错误响应（no-store）必须带。

// 集成测试目标只含测试代码，断言直接解包。
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use auction_http::{
    AdminApiState, PublicApiState, SurfacePages, WebScriptPolicy, admin_router_with_pages,
    public_router_with_pages,
};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use rust_embed::RustEmbed;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

/// 夹具里的两个 build：旧壳与新版 Solid。构建号只用明显的假值。
const LEGACY_BUILD: &str = "legacy-fixture-1";
const SOLID_BUILD: &str = "solid-fixture-2";
const LEGACY_INDEX: &[u8] = b"<html>legacy shell fixture</html>";
const SOLID_INDEX: &[u8] = b"<html>solid page fixture</html>";

/// 期望的两条策略字节：独立于生产常量写出，生产常量漂移会让本测试失败。
const DIOXUS_CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-eval' \
                          'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                          object-src 'none'; base-uri 'self'; form-action 'self'; \
                          frame-ancestors 'none'";
const SOLID_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' \
                         'unsafe-inline'; object-src 'none'; base-uri 'self'; \
                         form-action 'self'; frame-ancestors 'none'";

/// 本二进制内嵌的页面不会被用到（两个入口都指向挂载的 build）；只为满足 `RustEmbed` 类型参数。
#[derive(RustEmbed)]
#[folder = "tests/assets-fixture/"]
struct EmbeddedFixture;

type Files = Vec<(&'static str, &'static [u8])>;

fn legacy_files() -> Files {
    vec![
        ("index.html", LEGACY_INDEX),
        ("assets/app-dxh01234567.js", b"legacy-js"),
        ("assets/app-dxh01234567.js.br", b"legacy-js-br"),
        ("assets/app_bg-dxh2.wasm", b"\0asm-legacy"),
    ]
}

fn solid_files() -> Files {
    vec![("index.html", SOLID_INDEX), ("assets/index-B2UikXgQ.js", b"solid-js")]
}

fn sha(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn hex(data: &[u8]) -> String {
    sha(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 演练的两端；各自独立的挂载目录、指针与路由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Public,
    Admin,
}

const SURFACES: [Surface; 2] = [Surface::Public, Surface::Admin];

impl Surface {
    fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
        }
    }
}

/// 指针所处的演练状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pointer {
    New,
    Legacy,
}

impl Pointer {
    fn build(self) -> &'static str {
        match self {
            Self::New => SOLID_BUILD,
            Self::Legacy => LEGACY_BUILD,
        }
    }

    fn index(self) -> &'static [u8] {
        match self {
            Self::New => SOLID_INDEX,
            Self::Legacy => LEGACY_INDEX,
        }
    }

    fn stack(self) -> &'static str {
        match self {
            Self::New => "solid",
            Self::Legacy => "dioxus",
        }
    }

    fn csp(self) -> &'static str {
        match self {
            Self::New => SOLID_CSP,
            Self::Legacy => DIOXUS_CSP,
        }
    }
}

/// 测试专用临时挂载根；析构时清理。
struct Deployment {
    root: PathBuf,
}

impl Deployment {
    /// 旧壳字节按挂载布局物化为只读 build，新版 Solid build 同样落盘；此时尚无入口指针。
    fn materialize() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "auction-web-switch-drill-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let deployment = Self { root };
        for surface in SURFACES {
            fs::create_dir_all(deployment.surface(surface)).expect("建 surface 目录");
            deployment.write_build(surface, LEGACY_BUILD, "dioxus", &legacy_files());
            deployment.write_build(surface, SOLID_BUILD, "solid", &solid_files());
        }
        deployment
    }

    fn surface(&self, surface: Surface) -> PathBuf {
        self.root.join(surface.name())
    }

    fn write_build(&self, surface: Surface, build_id: &str, stack: &str, files: &Files) {
        let build = self.surface(surface).join(build_id);
        let mut manifest = format!(
            "schema=auction-web-build/1\nsurface={}\nbuild_id={build_id}\nstack={stack}\n\
             payload_sha256={}\n\n",
            surface.name(),
            hex(build_id.as_bytes())
        );
        for (path, data) in files {
            let target = build.join("files").join(path);
            fs::create_dir_all(target.parent().expect("父目录")).expect("建目录");
            fs::write(&target, data).expect("写文件");
            let kind = if path.ends_with(".br") { "br-entity" } else { "identity" };
            manifest.push_str(&format!("{}  ./{path}  {kind}\n", hex(data)));
        }
        fs::write(build.join("MANIFEST.v1"), manifest).expect("写清单");
    }

    /// 同目录临时文件加 rename：与 CLI 切换的原子替换同一形状。独立入口永远是新版 Solid。
    fn switch(&self, surface: Surface, to: Pointer) {
        let text = format!(
            "schema=auction-web-entry/1\nsurface={}\nbuild_id={}\nstack={}\n\
             script_policy={}\nentry_sha256={}\npayload_sha256={}\n\
             independent_build_id={SOLID_BUILD}\nindependent_payload_sha256={}\n",
            surface.name(),
            to.build(),
            to.stack(),
            to.stack(),
            hex(to.index()),
            hex(to.build().as_bytes()),
            hex(SOLID_BUILD.as_bytes()),
        );
        let temporary = self.surface(surface).join(".ENTRY.v1.tmp");
        fs::write(&temporary, text).expect("写临时指针");
        fs::rename(&temporary, self.surface(surface).join("ENTRY.v1")).expect("替换指针");
    }

    /// 重新构造页面与路由：等价于切换指针后重启服务（指针只在启动时读取一次）。
    fn restart(&self, surface: Surface) -> Router {
        let root = Path::new(&self.root);
        match surface {
            Surface::Public => {
                let pages = SurfacePages::with_public_mount::<EmbeddedFixture>(
                    WebScriptPolicy::Solid,
                    root,
                )
                .expect("Public 挂载应加载成功");
                let state = PublicApiState::default();
                let boundary = state.http_boundary();
                let gate = state.anonymous_read_gate();
                public_router_with_pages(env!("CARGO_PKG_VERSION"), boundary, gate, Arc::new(pages))
                    .with_state(state)
            }
            Surface::Admin => {
                let pages =
                    SurfacePages::with_admin_mount::<EmbeddedFixture>(WebScriptPolicy::Solid, root)
                        .expect("Admin 挂载应加载成功");
                let state = AdminApiState::default();
                let boundary = state.http_boundary();
                admin_router_with_pages(env!("CARGO_PKG_VERSION"), boundary, Arc::new(pages))
                    .with_state(state)
            }
        }
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

/// 一次响应里与演练有关的全部事实。
#[derive(Clone, Debug)]
struct Observation {
    status: u16,
    build: String,
    independent_build: String,
    csp: String,
    etag: String,
    cache_control: String,
    vary: String,
    body: Vec<u8>,
}

async fn probe(app: &Router, uri: &str, headers: &[(&str, &str)]) -> Observation {
    let mut builder = Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response =
        app.clone().oneshot(builder.body(Body::empty()).expect("请求")).await.expect("响应");
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let observation = Observation {
        status: response.status().as_u16(),
        build: header("x-auction-build"),
        independent_build: header("x-auction-build-independent"),
        csp: header("content-security-policy"),
        etag: header("etag"),
        cache_control: header("cache-control"),
        vary: header("vary"),
        body: Vec::new(),
    };
    let body = to_bytes(response.into_body(), 1 << 20).await.expect("正文").to_vec();
    Observation { body, ..observation }
}

/// 逐响应校验的期望：全部来自夹具与演练状态，不读服务端的任何常量。
struct Expected {
    status: u16,
    /// 构建号响应头只在 no-store 响应上发送：入口响应（页面靠入口 HTML 摘要识别 build）为 None，
    /// 此时两个构建号头都必须不存在。
    build: Option<&'static str>,
    /// 响应必须带的 `Cache-Control`（API 错误响应为 no-store）；入口响应的 no-cache 随 html 核对。
    no_store: bool,
    html: Option<&'static [u8]>,
    csp: &'static str,
    etag: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Violation {
    Status,
    Build,
    IndependentBuild,
    EntryBuildHeader,
    NoStore,
    Policy,
    Body,
    ETag,
    CacheControl,
    Vary,
}

/// 策略必须逐字节相等；放宽它（例如只比前缀或恒真）正是本演练要防的缺陷。
fn policy_matches(expected: &str, observed: &str) -> bool {
    expected == observed
}

/// 校验一个响应：状态、构建号（指针构建与独立构建）、策略字节、入口 HTML 摘要、验证器与缓存头。
fn judge(expected: &Expected, observed: &Observation) -> Result<(), Violation> {
    if observed.status != expected.status {
        return Err(Violation::Status);
    }
    match expected.build {
        Some(build) => {
            if observed.build != build {
                return Err(Violation::Build);
            }
            if observed.independent_build != SOLID_BUILD {
                return Err(Violation::IndependentBuild);
            }
        }
        None => {
            if !observed.build.is_empty() || !observed.independent_build.is_empty() {
                return Err(Violation::EntryBuildHeader);
            }
        }
    }
    if expected.no_store && observed.cache_control != "no-store" {
        return Err(Violation::NoStore);
    }
    if !policy_matches(expected.csp, &observed.csp) {
        return Err(Violation::Policy);
    }
    if let Some(html) = expected.html {
        if sha(&observed.body) != sha(html) {
            return Err(Violation::Body);
        }
        if observed.cache_control != "no-cache" {
            return Err(Violation::CacheControl);
        }
        if observed.vary != "Accept-Encoding" {
            return Err(Violation::Vary);
        }
    }
    if let Some(etag) = &expected.etag
        && &observed.etag != etag
    {
        return Err(Violation::ETag);
    }
    Ok(())
}

/// 页面类别：默认入口随指针，独立入口永远是新版 Solid。
fn entry_expectation(
    state: Pointer,
    independent: bool,
    status: u16,
    etag: Option<String>,
) -> Expected {
    let page = if independent { Pointer::New } else { state };
    Expected {
        status,
        build: None,
        no_store: false,
        html: (status == 200).then(|| page.index()),
        csp: page.csp(),
        etag,
    }
}

const DEFAULT_ENTRY: &str = "/";
const INDEPENDENT_ENTRY: &str = "/consignment";

/// 一个状态下的全部逐响应核对，返回两个入口的 200 观测（供错配用例取材）。
async fn verify_state(app: &Router, state: Pointer) -> BTreeMap<&'static str, Observation> {
    let mut seen = BTreeMap::new();
    for (uri, independent) in [(DEFAULT_ENTRY, false), (INDEPENDENT_ENTRY, true)] {
        let fresh = probe(app, uri, &[]).await;
        judge(&entry_expectation(state, independent, 200, None), &fresh)
            .unwrap_or_else(|violation| panic!("{state:?} {uri} 200：{violation:?}"));
        // 重新验证：同一验证器命中 304，且 304 仍带同一版本的策略与构建号。
        let revalidated = probe(app, uri, &[("if-none-match", &fresh.etag)]).await;
        judge(&entry_expectation(state, independent, 304, Some(fresh.etag.clone())), &revalidated)
            .unwrap_or_else(|violation| panic!("{state:?} {uri} 304：{violation:?}"));
        seen.insert(uri, fresh);
    }
    // 非页面响应（API 未知路由）永远是 Solid 严格策略，不因旧壳放宽。
    let api = probe(app, "/api/v1/no-such-route", &[]).await;
    judge(
        &Expected {
            status: 404,
            build: Some(state.build()),
            no_store: true,
            html: None,
            csp: SOLID_CSP,
            etag: None,
        },
        &api,
    )
    .unwrap_or_else(|violation| panic!("{state:?} API 404：{violation:?}"));
    seen
}

/// 正例：旧壳字节物化 → 指针切到新版 → 回切 → 再切；每步重启后逐响应核对，
/// 回切时 `/consignment` 仍是 Solid HTML 加 Solid 策略，旧壳的全部清单对象按旧策略取回。
#[tokio::test]
async fn cold_rollback_drill_every_response_matches_its_build_html_and_policy() {
    let deployment = Deployment::materialize();
    for surface in SURFACES {
        drill_surface(&deployment, surface).await;
    }
}

/// 一端的完整演练：新版 → 回切旧壳 → 再切新版，每次切换后重启并逐响应核对。
async fn drill_surface(deployment: &Deployment, surface: Surface) {
    deployment.switch(surface, Pointer::New);
    let new_app = deployment.restart(surface);
    verify_state(&new_app, Pointer::New).await;

    // 切到旧壳后不重启：旧进程继续服务启动时读到的指针，没有热重载的半切换状态。
    deployment.switch(surface, Pointer::Legacy);
    let stale = probe(&new_app, DEFAULT_ENTRY, &[]).await;
    judge(&entry_expectation(Pointer::New, false, 200, None), &stale).expect("未重启仍是新版");

    let legacy_app = deployment.restart(surface);
    let seen = verify_state(&legacy_app, Pointer::Legacy).await;
    assert_ne!(
        seen[DEFAULT_ENTRY].body, seen[INDEPENDENT_ENTRY].body,
        "回切期间两个入口是不同 HTML"
    );

    // 旧页面的每个清单对象：原文摘要、旧策略；预压缩副本按 br 协商原样发出。
    for (path, data) in legacy_files().into_iter().filter(|(path, _)| !path.ends_with(".br")) {
        let uri = format!("/{path}");
        let asset = probe(&legacy_app, &uri, &[]).await;
        assert_eq!(asset.status, 200, "{uri}");
        assert_eq!(sha(&asset.body), sha(data), "{uri}");
        assert_eq!(asset.csp, DIOXUS_CSP, "{uri}");
    }
    let brotli =
        probe(&legacy_app, "/assets/app-dxh01234567.js", &[("accept-encoding", "br")]).await;
    assert_eq!(brotli.body, b"legacy-js-br");

    deployment.switch(surface, Pointer::New);
    let again = deployment.restart(surface);
    verify_state(&again, Pointer::New).await;
}

/// 反例 1：旧 HTML 配 Solid 策略（页面层把全站统一成新栈的 CSP）必须被判策略错配。
#[tokio::test]
async fn legacy_html_with_solid_csp_is_rejected() {
    let deployment = Deployment::materialize();
    deployment.switch(Surface::Public, Pointer::Legacy);
    let app = deployment.restart(Surface::Public);
    let genuine = probe(&app, DEFAULT_ENTRY, &[]).await;
    let expected = entry_expectation(Pointer::Legacy, false, 200, None);
    judge(&expected, &genuine).expect("对照组：真实响应必须通过，否则下面的失败不说明问题");

    let mismatched = Observation { csp: SOLID_CSP.to_owned(), ..genuine };
    assert_eq!(judge(&expected, &mismatched), Err(Violation::Policy));
}

/// 反例 2：回切期间新版 `/consignment` 页面配旧 Dioxus 策略（策略随指针走而不随页面走）必须被拒。
#[tokio::test]
async fn solid_rights_page_with_legacy_csp_is_rejected() {
    let deployment = Deployment::materialize();
    deployment.switch(Surface::Public, Pointer::Legacy);
    let app = deployment.restart(Surface::Public);
    let genuine = probe(&app, INDEPENDENT_ENTRY, &[]).await;
    let expected = entry_expectation(Pointer::Legacy, true, 200, None);
    judge(&expected, &genuine).expect("对照组：真实响应必须通过");

    let mismatched = Observation { csp: DIOXUS_CSP.to_owned(), ..genuine };
    assert_eq!(judge(&expected, &mismatched), Err(Violation::Policy));
}

/// 边缘缓存替身：按验证器应答条件请求，复用它缓存的另一版本的 304 头部（含旧策略）；
/// 304 按协议回显客户端发来的验证器，所以验证器本身看不出错配，只有策略字节能暴露。
fn cdn_replays_cached_304(cached: &Observation, sent_etag: &str) -> Observation {
    Observation { etag: sent_etag.to_owned(), body: Vec::new(), ..cached.clone() }
}

/// 反例 3：304 / CDN 复用错版本策略必须被拒；真实源站对跨版本的验证器不会返回 304。
#[tokio::test]
async fn cdn_reusing_a_304_from_the_other_policy_is_rejected() {
    let deployment = Deployment::materialize();
    deployment.switch(Surface::Public, Pointer::Legacy);
    let app = deployment.restart(Surface::Public);
    let legacy_page = probe(&app, DEFAULT_ENTRY, &[]).await;
    let solid_page = probe(&app, INDEPENDENT_ENTRY, &[]).await;
    assert_ne!(legacy_page.etag, solid_page.etag, "同一指针下两种策略的页面验证器必须不同");

    // 源站：拿旧壳的验证器问 Solid 页面，得到完整的 200 与 Solid 策略，而不是 304。
    let crossed = probe(&app, INDEPENDENT_ENTRY, &[("if-none-match", &legacy_page.etag)]).await;
    judge(&entry_expectation(Pointer::Legacy, true, 200, None), &crossed).expect("源站不复用");

    // 替身：缓存了旧壳页面的 304，再用它回答 Solid 页面的条件请求。
    let legacy_304 = probe(&app, DEFAULT_ENTRY, &[("if-none-match", &legacy_page.etag)]).await;
    assert_eq!(legacy_304.status, 304);
    let replayed = cdn_replays_cached_304(&legacy_304, &solid_page.etag);
    let expected = entry_expectation(Pointer::Legacy, true, 304, Some(solid_page.etag.clone()));
    assert_eq!(judge(&expected, &replayed), Err(Violation::Policy));
}

/// 反例 4：入口响应带了构建号头（服务端行为漂移，入口是 no-cache 的可重放响应）必须被拒；
/// API 错误响应不是 no-store、或缺构建号头同样被拒。两端各验一次，对照组先通过。
#[tokio::test]
async fn entry_build_headers_and_api_error_cache_control_are_enforced_on_both_surfaces() {
    let deployment = Deployment::materialize();
    for surface in SURFACES {
        deployment.switch(surface, Pointer::Legacy);
        let app = deployment.restart(surface);

        let entry = probe(&app, DEFAULT_ENTRY, &[]).await;
        let expected = entry_expectation(Pointer::Legacy, false, 200, None);
        judge(&expected, &entry).expect("对照组：入口响应不带构建号头");
        let with_build = Observation { build: LEGACY_BUILD.to_owned(), ..entry.clone() };
        assert_eq!(judge(&expected, &with_build), Err(Violation::EntryBuildHeader));
        let with_independent = Observation { independent_build: SOLID_BUILD.to_owned(), ..entry };
        assert_eq!(judge(&expected, &with_independent), Err(Violation::EntryBuildHeader));

        let api = probe(&app, "/api/v1/no-such-route", &[]).await;
        let expected = Expected {
            status: 404,
            build: Some(Pointer::Legacy.build()),
            no_store: true,
            html: None,
            csp: SOLID_CSP,
            etag: None,
        };
        judge(&expected, &api).expect("对照组：API 错误响应 no-store 并带两个构建号头");
        let cacheable = Observation { cache_control: "no-cache".to_owned(), ..api };
        assert_eq!(judge(&expected, &cacheable), Err(Violation::NoStore));
    }
}
