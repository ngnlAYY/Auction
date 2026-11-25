//! 静态挂载加载：读取入口指针，解析默认与独立两个 build，启动时一次完成全部核对。
//!
//! 失败一律返回 `WebMountError` 由装配根终止启动：不降级回内嵌页面、不跳过未通过的文件。
//! 入口指针只在启动时读取一次，切换指针后须重启服务才生效。

mod directory;
mod entry;
mod error;
#[cfg(test)]
mod tests;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use rust_embed::RustEmbed;

use self::directory::{load_build, require_directory};
use self::entry::EntryPointer;
use super::source::{AssetSource, EmbeddedSource};
use crate::WebScriptPolicy;
use crate::server::build_identity::build_id;

pub use self::error::WebMountError;

const ENTRY_FILE: &str = "ENTRY.v1";
const MAX_ENTRY_BYTES: u64 = 4096;
const INDEX_HTML: &str = "index.html";

/// 一个已解析、已核对的 build：构建号、脚本策略与页面字节三者来自同一来源。
pub(super) struct ResolvedBuild {
    pub(super) build_id: String,
    pub(super) policy: WebScriptPolicy,
    pub(super) source: Arc<dyn AssetSource>,
}

pub(super) struct ResolvedMount {
    pub(super) default_build: ResolvedBuild,
    /// 与默认入口是同一个 build 时为 `None`，不重复持有也不重复查表。
    pub(super) independent_build: Option<ResolvedBuild>,
}

/// 读取并完整验证 `<web_root>/<surface>`；`A` 是本二进制内嵌的页面，构建号等于入口指针
/// 所指 build 时直接使用内嵌字节，不依赖挂载目录。
pub(super) fn load_surface<A: RustEmbed + 'static>(
    embedded_policy: WebScriptPolicy,
    web_root: &Path,
    surface: &str,
) -> Result<ResolvedMount, WebMountError> {
    let surface_root = web_root.join(surface);
    require_directory(&surface_root)?;
    let pointer = read_pointer(&surface_root, surface)?;

    let (default_build, index_sha256) = resolve::<A>(
        &pointer.build_id,
        &pointer.payload_sha256,
        embedded_policy,
        &surface_root,
        surface,
    )?;
    if default_build.policy != pointer.policy {
        return Err(WebMountError::PolicyMismatch("入口 build 的栈与指针不一致"));
    }
    if index_sha256 != pointer.entry_sha256 {
        return Err(WebMountError::DigestMismatch("入口 index.html 与 entry_sha256".to_owned()));
    }

    if pointer.independent_build_id == pointer.build_id {
        // 正常 Solid 运行：默认入口即独立入口，旧壳目录不参与。
        if pointer.independent_payload_sha256 != pointer.payload_sha256 {
            return Err(WebMountError::Entry("同一 build 的两个 payload_sha256 不一致"));
        }
        if default_build.policy != WebScriptPolicy::Solid {
            return Err(WebMountError::PolicyMismatch("独立入口必须是 Solid"));
        }
        return Ok(ResolvedMount { default_build, independent_build: None });
    }

    let (independent_build, _) = resolve::<A>(
        &pointer.independent_build_id,
        &pointer.independent_payload_sha256,
        embedded_policy,
        &surface_root,
        surface,
    )?;
    if independent_build.policy != WebScriptPolicy::Solid {
        return Err(WebMountError::PolicyMismatch("独立入口必须是 Solid"));
    }
    reject_conflicts(&default_build, &independent_build)?;
    Ok(ResolvedMount { default_build, independent_build: Some(independent_build) })
}

fn read_pointer(surface_root: &Path, surface: &str) -> Result<EntryPointer, WebMountError> {
    let path = surface_root.join(ENTRY_FILE);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|source| WebMountError::Io { path: path.clone(), source })?;
    if !metadata.is_file() || metadata.len() > MAX_ENTRY_BYTES {
        return Err(WebMountError::Entry("不是普通文件或过大"));
    }
    let text = fs::read_to_string(&path).map_err(|source| WebMountError::Io { path, source })?;
    EntryPointer::parse(&text, surface)
}

/// 构建号等于本二进制的构建号时取内嵌页面，否则从挂载目录加载；同时返回入口 HTML 摘要。
fn resolve<A: RustEmbed + 'static>(
    wanted_build_id: &str,
    payload_sha256: &str,
    embedded_policy: WebScriptPolicy,
    surface_root: &Path,
    surface: &str,
) -> Result<(ResolvedBuild, String), WebMountError> {
    if wanted_build_id == build_id() {
        let source = EmbeddedSource::<A>::new();
        let index = source
            .get(INDEX_HTML)
            .ok_or_else(|| WebMountError::FileMissing(INDEX_HTML.to_owned()))?;
        let index_sha256 = auction_application::core::lower_hex(&index.sha256);
        let build = ResolvedBuild {
            build_id: wanted_build_id.to_owned(),
            policy: embedded_policy,
            source: Arc::new(source),
        };
        return Ok((build, index_sha256));
    }
    let loaded = load_build(surface_root, surface, wanted_build_id, payload_sha256)?;
    let build = ResolvedBuild {
        build_id: wanted_build_id.to_owned(),
        policy: loaded.policy,
        source: Arc::new(loaded.source),
    };
    Ok((build, loaded.index_sha256))
}

/// 同一 URL 不能在两个 build 里是不同字节：资源查找先默认后独立，异字节会让页面取到错版本。
/// 入口文档 `index.html` 按入口类别选择，不经路径查找，因此不参与比较。
fn reject_conflicts(
    default_build: &ResolvedBuild,
    independent_build: &ResolvedBuild,
) -> Result<(), WebMountError> {
    for path in default_build.source.paths() {
        if path == INDEX_HTML {
            continue;
        }
        let (Some(left), Some(right)) =
            (default_build.source.get(&path), independent_build.source.get(&path))
        else {
            continue;
        };
        if left.sha256 != right.sha256 {
            return Err(WebMountError::Conflict(path.into_owned()));
        }
    }
    Ok(())
}
