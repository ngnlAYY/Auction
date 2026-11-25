//! 一个 surface 的页面集合：来源、构建号与脚本策略三者绑定为 `PageBuild`，构造后不可拆开。

use std::path::Path;
use std::sync::Arc;

use rust_embed::RustEmbed;

use super::independent::EntryClass;
use super::mount::{ResolvedBuild, WebMountError, load_surface};
use super::source::{AssetFile, AssetSource, EmbeddedSource};
use crate::WebScriptPolicy;
use crate::server::build_identity::{BuildReport, build_id};

/// 一个 build 的页面：来源、构建号与脚本策略。
pub(crate) struct PageBuild {
    build_id: String,
    script_policy: WebScriptPolicy,
    source: Arc<dyn AssetSource>,
    /// 配置了挂载时，页面 `ETag` 混入该策略标识，使“同一 HTML、不同策略”不会被 304 复用；
    /// 纯内嵌时为 `None`，响应字节与历史一致。
    etag_policy_tag: Option<&'static str>,
}

impl PageBuild {
    fn mounted(build: ResolvedBuild) -> Self {
        Self {
            build_id: build.build_id,
            script_policy: build.policy,
            source: build.source,
            etag_policy_tag: Some(policy_tag(build.policy)),
        }
    }

    pub(crate) fn script_policy(&self) -> WebScriptPolicy {
        self.script_policy
    }

    pub(crate) fn etag_policy_tag(&self) -> Option<&'static str> {
        self.etag_policy_tag
    }

    pub(crate) fn get(&self, relative_path: &str) -> Option<AssetFile> {
        self.source.get(relative_path)
    }

    pub(crate) fn favicon_path(&self) -> Option<String> {
        self.source.favicon_path()
    }
}

fn policy_tag(policy: WebScriptPolicy) -> &'static str {
    match policy {
        WebScriptPolicy::Dioxus => "dioxus",
        WebScriptPolicy::Solid => "solid",
    }
}

/// 一个 surface 的全部页面：默认入口所指 build，加上独立入口（与默认不同时才存在）。
pub struct SurfacePages {
    /// 非页面响应（API、错误页、重定向）使用的策略：装配根声明的本二进制自身栈。
    surface_policy: WebScriptPolicy,
    default_entry: PageBuild,
    independent_entry: Option<PageBuild>,
    /// 是否来自静态挂载；纯内嵌时只报告进程构建号，不发独立构建号头（E3 再补充）。
    mounted: bool,
}

impl SurfacePages {
    /// 纯内嵌：默认与独立入口都是内嵌 build，行为与没有挂载功能时字节一致。
    pub fn embedded<A: RustEmbed + 'static>(policy: WebScriptPolicy) -> Self {
        Self {
            surface_policy: policy,
            default_entry: PageBuild {
                build_id: build_id().to_owned(),
                script_policy: policy,
                source: Arc::new(EmbeddedSource::<A>::new()),
                etag_policy_tag: None,
            },
            independent_entry: None,
            mounted: false,
        }
    }

    /// Public：读取并完整验证 `<web_root>/public`；任何不一致都返回错误，调用方应终止启动。
    pub fn with_public_mount<A: RustEmbed + 'static>(
        embedded_policy: WebScriptPolicy,
        web_root: &Path,
    ) -> Result<Self, WebMountError> {
        Self::with_mount::<A>(embedded_policy, web_root, "public")
    }

    /// Admin：读取并完整验证 `<web_root>/admin`；首次设置页不经此路径，永远走内嵌。
    pub fn with_admin_mount<A: RustEmbed + 'static>(
        embedded_policy: WebScriptPolicy,
        web_root: &Path,
    ) -> Result<Self, WebMountError> {
        Self::with_mount::<A>(embedded_policy, web_root, "admin")
    }

    fn with_mount<A: RustEmbed + 'static>(
        embedded_policy: WebScriptPolicy,
        web_root: &Path,
        surface: &str,
    ) -> Result<Self, WebMountError> {
        let mount = load_surface::<A>(embedded_policy, web_root, surface)?;
        Ok(Self {
            surface_policy: embedded_policy,
            default_entry: PageBuild::mounted(mount.default_build),
            independent_entry: mount.independent_build.map(PageBuild::mounted),
            mounted: true,
        })
    }

    /// 非页面响应使用的 surface 默认脚本策略。
    pub(crate) fn surface_policy(&self) -> WebScriptPolicy {
        self.surface_policy
    }

    /// 报告给浏览器的构建号：默认入口所指 build 与独立入口所指 build。
    pub(crate) fn build_report(&self) -> BuildReport {
        if !self.mounted {
            return BuildReport::process();
        }
        let independent = self.independent_entry.as_ref().unwrap_or(&self.default_entry);
        BuildReport::new(&self.default_entry.build_id, &independent.build_id)
            .unwrap_or_else(BuildReport::process)
    }

    /// 页面请求按入口类别取 build。
    pub(super) fn entry(&self, class: EntryClass) -> &PageBuild {
        match class {
            EntryClass::Default => &self.default_entry,
            EntryClass::Independent => {
                self.independent_entry.as_ref().unwrap_or(&self.default_entry)
            }
        }
    }

    /// 精确资源先查默认入口再查独立入口；两个 build 的同 URL 字节在加载时已核对一致。
    pub(super) fn find(&self, relative_path: &str) -> Option<(&PageBuild, AssetFile)> {
        [Some(&self.default_entry), self.independent_entry.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|build| build.get(relative_path).map(|file| (build, file)))
    }

    /// 站点图标别名：先默认入口再独立入口。
    pub(super) fn find_favicon(&self) -> Option<(&PageBuild, String, AssetFile)> {
        [Some(&self.default_entry), self.independent_entry.as_ref()].into_iter().flatten().find_map(
            |build| {
                let path = build.favicon_path()?;
                let file = build.get(&path)?;
                Some((build, path, file))
            },
        )
    }
}
