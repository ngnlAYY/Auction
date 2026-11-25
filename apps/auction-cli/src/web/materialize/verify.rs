//! 取回结果与磁盘 build 的核对：逐文件摘要、路径全集、数量与大小上限，以及源站身份。
//!
//! 任何不一致都失败：HTML fallback（缺失路径返回 200 入口页）、自动解压后的字节、同名异字节、
//! 漏一个 chunk 与清单外文件，都会在这里变成错误，调用方因此零安装、指针不变。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Context;

use super::layout::{
    BuildManifest, FILES_DIR, INDEX_HTML, MANIFEST_FILE, MAX_FILE_BYTES, MAX_FILES,
    MAX_MANIFEST_BYTES, MAX_TOTAL_BYTES, Surface, validate_relative_path,
};
use crate::support::digest::sha256_bytes;
use crate::support::files;

/// 单个取回文件的字节必须等于清单摘要。
pub(super) fn verify_file(path: &str, expected_sha256: &str, bytes: &[u8]) -> anyhow::Result<()> {
    if sha256_bytes(bytes) != expected_sha256 {
        anyhow::bail!("取回的文件摘要与清单不符：{path}");
    }
    Ok(())
}

/// 旧 Dioxus 页面的 CSP，与 `auction-http` 的 `DIOXUS_CONTENT_SECURITY_POLICY` 逐字节相同。
/// 该常量是私有的，CLI 不为它依赖整个 HTTP crate；测试对照真实的安全头输出防止两处漂移。
pub(super) const DIOXUS_CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "object-src 'none'; ",
    "base-uri 'self'; ",
    "form-action 'self'; ",
    "frame-ancestors 'none'",
);

/// 入口页响应的 CSP 必须逐字节等于旧 Dioxus 的策略：不等说明源站不是旧 Dioxus 二进制
/// （例如 Solid 或被前置代理改写），网络上看到的值不作为信任来源，只用来拒绝。
pub(super) fn verify_index_policy(policy: Option<&str>) -> anyhow::Result<()> {
    match policy {
        Some(DIOXUS_CONTENT_SECURITY_POLICY) => Ok(()),
        Some(_) => {
            anyhow::bail!("入口页的 CSP 不等于旧 Dioxus 页面的策略：源站不是旧 Dioxus 二进制")
        }
        None => anyhow::bail!("入口页响应缺少 Content-Security-Policy"),
    }
}

/// 已核对的磁盘 build：清单与入口 HTML 摘要。
#[derive(Debug)]
pub(super) struct BuildFacts {
    pub(super) manifest: BuildManifest,
    pub(super) index_sha256: String,
}

/// 逐项核对 `<surface_root>/<build_id>`：清单头与目录一致、路径集合与磁盘完全相同、每个文件的
/// 摘要等于清单；符号链接与特殊文件一律拒绝。
pub(super) fn verify_build_dir(
    surface_root: &Path,
    surface: Surface,
    build_id: &str,
) -> anyhow::Result<BuildFacts> {
    let build_root = surface_root.join(build_id);
    require_real_directory(&build_root)?;
    let text = files::read_bounded(&build_root.join(MANIFEST_FILE), MAX_MANIFEST_BYTES)?;
    let text = String::from_utf8(text).context("清单不是 UTF-8")?;
    let manifest = BuildManifest::parse(&text)?;
    if manifest.surface != surface || manifest.build_id != build_id {
        anyhow::bail!("清单的 surface 或 build_id 与所在目录不符：{build_id}");
    }
    if manifest.files.len() > MAX_FILES {
        anyhow::bail!("清单文件数超过上限：{build_id}");
    }
    let listed: BTreeSet<&str> = manifest.files.iter().map(|file| file.path.as_str()).collect();
    if listed.len() != manifest.files.len() {
        anyhow::bail!("清单路径重复登记：{build_id}");
    }
    if !listed.contains(INDEX_HTML) {
        anyhow::bail!("清单缺少 {INDEX_HTML}：{build_id}");
    }

    let files_root = build_root.join(FILES_DIR);
    require_real_directory(&files_root)?;
    let mut on_disk = Vec::new();
    walk(&files_root, "", &mut on_disk)?;
    if let Some(extra) = on_disk.iter().find(|path| !listed.contains(path.as_str())) {
        anyhow::bail!("build 目录存在清单外的文件：{extra}");
    }
    let mut total = 0_u64;
    for file in &manifest.files {
        validate_relative_path(&file.path)?;
        let bytes = files::read_bounded(&files_root.join(&file.path), MAX_FILE_BYTES)
            .with_context(|| format!("清单登记的文件缺失或不可读：{}", file.path))?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            anyhow::bail!("build 总字节超过上限：{build_id}");
        }
        verify_file(&file.path, &file.sha256, &bytes)?;
    }
    let index_sha256 = manifest
        .files
        .iter()
        .find(|file| file.path == INDEX_HTML)
        .map(|file| file.sha256.clone())
        .context("清单缺少入口页")?;
    Ok(BuildFacts { manifest, index_sha256 })
}

/// 两个 build 在同一路径上字节必须相同（入口页按入口类别选择，不参与比较）。
pub(super) fn reject_conflicts(left: &BuildManifest, right: &BuildManifest) -> anyhow::Result<()> {
    let right_digests: BTreeMap<&str, &str> =
        right.files.iter().map(|file| (file.path.as_str(), file.sha256.as_str())).collect();
    for file in &left.files {
        if file.path == INDEX_HTML {
            continue;
        }
        if right_digests.get(file.path.as_str()).is_some_and(|digest| *digest != file.sha256) {
            anyhow::bail!("两个 build 在同一路径上字节不同：{}", file.path);
        }
    }
    Ok(())
}

/// 目录必须是真实目录：符号链接可以把挂载根指到任意位置，一律拒绝。
pub(super) fn require_real_directory(path: &Path) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("读取目录元数据失败：{}", path.display()))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        anyhow::bail!("必须是真实目录（不得是符号链接或文件）：{}", path.display())
    }
}

/// 递归列出 `files/` 下的普通文件；符号链接与特殊文件直接失败。
fn walk(directory: &Path, prefix: &str, found: &mut Vec<String>) -> anyhow::Result<()> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("读取目录失败：{}", directory.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("读取目录项失败：{}", directory.display()))?;
        let name = entry.file_name();
        let name = name.to_str().context("目录项名称不是 UTF-8")?;
        let relative = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        let file_type = entry.file_type().context("读取文件类型失败")?;
        if file_type.is_dir() {
            walk(&entry.path(), &relative, found)?;
        } else if file_type.is_file() {
            if found.len() >= MAX_FILES {
                anyhow::bail!("build 目录文件数超过上限");
            }
            found.push(relative);
        } else {
            anyhow::bail!("build 目录含符号链接或特殊文件：{relative}");
        }
    }
    Ok(())
}
