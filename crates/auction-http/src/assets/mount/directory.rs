//! 把一个已物化的 build 目录整体读入内存并逐文件核对清单。
//!
//! 目录布局：`<build>/MANIFEST.v1` 与 `<build>/files/<相对路径>`。核对在启动时一次完成：
//! 清单登记的路径集合就是全集（缺文件、多文件、符号链接、清单外内容都失败），每个文件的
//! SHA-256 必须等于清单；请求期只做内存查表，因此路径穿越在构造上不可能越出挂载根。

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::entry::BuildManifest;
use super::error::WebMountError;
use crate::WebScriptPolicy;
use crate::assets::source::{AssetFile, MountedSource};

const MANIFEST_FILE: &str = "MANIFEST.v1";
const FILES_DIR: &str = "files";
const INDEX_HTML: &str = "index.html";
/// 与 CLI 的归档成员上限同量级；超出说明清单或目录不是页面产物。
const MAX_FILES: usize = 4096;
const MAX_MANIFEST_BYTES: u64 = 4 << 20;
const MAX_FILE_BYTES: u64 = 64 << 20;
const MAX_TOTAL_BYTES: u64 = 512 << 20;

/// 已核对的 build：清单声明的策略、清单摘要、页面字节与入口 HTML 摘要。
pub(super) struct LoadedBuild {
    pub(super) policy: WebScriptPolicy,
    pub(super) source: MountedSource,
    pub(super) index_sha256: String,
}

pub(super) fn load_build(
    surface_root: &Path,
    surface: &str,
    build_id: &str,
    expected_payload_sha256: &str,
) -> Result<LoadedBuild, WebMountError> {
    let build_root = surface_root.join(build_id);
    require_directory(&build_root)?;
    let manifest_path = build_root.join(MANIFEST_FILE);
    let manifest_meta = fs::symlink_metadata(&manifest_path)
        .map_err(|source| WebMountError::Io { path: manifest_path.clone(), source })?;
    if !manifest_meta.is_file() || manifest_meta.len() > MAX_MANIFEST_BYTES {
        return Err(WebMountError::Manifest("清单不是普通文件或过大"));
    }
    let text = fs::read_to_string(&manifest_path)
        .map_err(|source| WebMountError::Io { path: manifest_path, source })?;
    let manifest = BuildManifest::parse(&text, surface, build_id)?;
    if manifest.payload_sha256 != expected_payload_sha256 {
        return Err(WebMountError::DigestMismatch(format!("{build_id}: payload_sha256")));
    }
    if manifest.files.len() > MAX_FILES {
        return Err(WebMountError::TooLarge("文件数量"));
    }

    let files_root = build_root.join(FILES_DIR);
    require_directory(&files_root)?;
    let mut listed = BTreeSet::new();
    for file in &manifest.files {
        validate_relative_path(&file.path)?;
        if !listed.insert(file.path.as_str()) {
            return Err(WebMountError::Manifest("路径重复登记"));
        }
        // br 实体只能附属于已登记的原文，孤立的预压缩副本没有可服务的语义。
        if file.brotli_entity
            && !file.path.strip_suffix(".br").is_some_and(|base| {
                manifest.files.iter().any(|other| !other.brotli_entity && other.path == base)
            })
        {
            return Err(WebMountError::Manifest("br 实体缺少对应原文"));
        }
    }
    if !listed.contains(INDEX_HTML) {
        return Err(WebMountError::FileMissing(INDEX_HTML.to_owned()));
    }
    let mut on_disk = Vec::new();
    walk(&files_root, "", &mut on_disk)?;
    if let Some(extra) = on_disk.iter().find(|path| !listed.contains(path.as_str())) {
        return Err(WebMountError::FileUnlisted(extra.clone()));
    }

    let mut files = HashMap::with_capacity(manifest.files.len());
    let mut total = 0_u64;
    for entry in &manifest.files {
        let path = files_root.join(&entry.path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(WebMountError::FileMissing(entry.path.clone()));
            }
            Err(source) => return Err(WebMountError::Io { path, source }),
        };
        if !metadata.is_file() {
            return Err(WebMountError::UnsafePath(entry.path.clone()));
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(WebMountError::TooLarge("单个文件"));
        }
        total += metadata.len();
        if total > MAX_TOTAL_BYTES {
            return Err(WebMountError::TooLarge("总字节"));
        }
        let data = fs::read(&path).map_err(|source| WebMountError::Io { path, source })?;
        let sha256: [u8; 32] = Sha256::digest(&data).into();
        if auction_application::core::lower_hex(&sha256) != entry.sha256 {
            return Err(WebMountError::DigestMismatch(entry.path.clone()));
        }
        files.insert(
            entry.path.clone(),
            AssetFile {
                data: data.into(),
                mimetype: mimetype(&entry.path).to_owned(),
                sha256,
                last_modified: None,
            },
        );
    }
    let index_sha256 = files
        .get(INDEX_HTML)
        .map(|index| auction_application::core::lower_hex(&index.sha256))
        .ok_or_else(|| WebMountError::FileMissing(INDEX_HTML.to_owned()))?;
    Ok(LoadedBuild { policy: manifest.policy, source: MountedSource::new(files), index_sha256 })
}

/// 目录必须是真实目录：符号链接可以把挂载根指到任意位置，一律拒绝。
pub(super) fn require_directory(path: &Path) -> Result<(), WebMountError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| WebMountError::Io { path: path.to_path_buf(), source })?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(WebMountError::UnsafePath(path.display().to_string()))
    }
}

/// 清单路径必须是干净的相对路径：非空段、无 `.` 与 `..`、无反斜线与控制字符。
fn validate_relative_path(path: &str) -> Result<(), WebMountError> {
    let unsafe_path = path.is_empty()
        || path.contains('\\')
        || path.bytes().any(|byte| byte.is_ascii_control())
        || path.split('/').any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
    if unsafe_path { Err(WebMountError::UnsafePath(path.to_owned())) } else { Ok(()) }
}

/// 递归列出 `files/` 下的全部普通文件；符号链接与特殊文件直接失败。
fn walk(directory: &Path, prefix: &str, found: &mut Vec<String>) -> Result<(), WebMountError> {
    let entries = fs::read_dir(directory)
        .map_err(|source| WebMountError::Io { path: directory.to_path_buf(), source })?;
    for entry in entries {
        let entry =
            entry.map_err(|source| WebMountError::Io { path: directory.to_path_buf(), source })?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| WebMountError::UnsafePath(entry.path().display().to_string()))?;
        let relative = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}/{name}") };
        let file_type =
            entry.file_type().map_err(|source| WebMountError::Io { path: entry.path(), source })?;
        if file_type.is_dir() {
            walk(&entry.path(), &relative, found)?;
        } else if file_type.is_file() {
            if found.len() >= MAX_FILES {
                return Err(WebMountError::TooLarge("文件数量"));
            }
            found.push(relative);
        } else {
            return Err(WebMountError::UnsafePath(relative));
        }
    }
    Ok(())
}

/// 按扩展名给出媒体类型；覆盖两套前端产物实际出现的类型，其余按二进制处理。
fn mimetype(path: &str) -> &'static str {
    let extension = path.rsplit_once('.').map_or("", |(_, extension)| extension);
    match extension {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "wasm" => "application/wasm",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}
