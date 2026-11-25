//! 入口指针 `ENTRY.v1` 与逐 build 清单 `MANIFEST.v1` 的文本格式解析。
//!
//! 两个文件都是 `key=value` 行，字段恰为且按固定顺序出现，LF 结尾；任何多余、缺失、
//! 重复或乱序都拒绝，不做宽容解析。

use super::error::WebMountError;
use crate::WebScriptPolicy;

const ENTRY_SCHEMA: &str = "auction-web-entry/1";
const MANIFEST_SCHEMA: &str = "auction-web-build/1";

/// 一个 surface 的入口指针：默认入口 build 与独立入口 build。
pub(super) struct EntryPointer {
    pub(super) build_id: String,
    /// 由 `stack` 推出并与冗余的 `script_policy` 字段互验，不是自由输入。
    pub(super) policy: WebScriptPolicy,
    pub(super) entry_sha256: String,
    pub(super) payload_sha256: String,
    pub(super) independent_build_id: String,
    pub(super) independent_payload_sha256: String,
}

/// 一个 build 的清单头与逐文件摘要。
pub(super) struct BuildManifest {
    pub(super) policy: WebScriptPolicy,
    pub(super) payload_sha256: String,
    pub(super) files: Vec<ManifestFile>,
}

pub(super) struct ManifestFile {
    pub(super) path: String,
    pub(super) sha256: String,
    /// `true` 表示该行是 `x.br`，保存的是 `Content-Encoding: br` 的原实体。
    pub(super) brotli_entity: bool,
}

fn stack(value: &str) -> Option<WebScriptPolicy> {
    match value {
        "dioxus" => Some(WebScriptPolicy::Dioxus),
        "solid" => Some(WebScriptPolicy::Solid),
        _ => None,
    }
}

fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// 取出恰为给定键序列的值；行数、键名或顺序不符返回 `None`。
fn exact_fields<'a, 'b>(
    lines: &mut impl Iterator<Item = &'a str>,
    keys: &[&'b str],
) -> Option<Vec<&'a str>> {
    keys.iter().map(|key| lines.next()?.strip_prefix(key)?.strip_prefix('=')).collect()
}

impl EntryPointer {
    pub(super) fn parse(text: &str, surface: &str) -> Result<Self, WebMountError> {
        let body = text.strip_suffix('\n').ok_or(WebMountError::Entry("缺少结尾换行"))?;
        let mut lines = body.split('\n');
        let fields = exact_fields(
            &mut lines,
            &[
                "schema",
                "surface",
                "build_id",
                "stack",
                "script_policy",
                "entry_sha256",
                "payload_sha256",
                "independent_build_id",
                "independent_payload_sha256",
            ],
        )
        .ok_or(WebMountError::Entry("字段缺失、重复或顺序不符"))?;
        if lines.next().is_some() {
            return Err(WebMountError::Entry("存在多余行"));
        }
        let [
            schema,
            entry_surface,
            build_id,
            entry_stack,
            script_policy,
            entry_sha256,
            payload_sha256,
            independent_build_id,
            independent_payload_sha256,
        ] = fields[..]
        else {
            return Err(WebMountError::Entry("字段数量不符"));
        };
        if schema != ENTRY_SCHEMA {
            return Err(WebMountError::Entry("schema 不符"));
        }
        if entry_surface != surface {
            return Err(WebMountError::Entry("surface 与挂载目录不符"));
        }
        let policy = stack(entry_stack).ok_or(WebMountError::Entry("stack 不在闭集内"))?;
        if stack(script_policy) != Some(policy) {
            return Err(WebMountError::PolicyMismatch("script_policy 与 stack 不一致"));
        }
        for build in [build_id, independent_build_id] {
            if !crate::server::build_identity::is_valid(build) {
                return Err(WebMountError::Entry("build_id 含非法字符或长度"));
            }
        }
        for digest in [entry_sha256, payload_sha256, independent_payload_sha256] {
            if !is_hex_digest(digest) {
                return Err(WebMountError::Entry("摘要不是 64 位小写十六进制"));
            }
        }
        Ok(Self {
            build_id: build_id.to_owned(),
            policy,
            entry_sha256: entry_sha256.to_owned(),
            payload_sha256: payload_sha256.to_owned(),
            independent_build_id: independent_build_id.to_owned(),
            independent_payload_sha256: independent_payload_sha256.to_owned(),
        })
    }
}

impl BuildManifest {
    /// `surface` 与 `build_id` 必须与它所在的目录一致，防止把别处的清单搬进来。
    pub(super) fn parse(text: &str, surface: &str, build_id: &str) -> Result<Self, WebMountError> {
        let body = text.strip_suffix('\n').ok_or(WebMountError::Manifest("缺少结尾换行"))?;
        let mut lines = body.split('\n');
        let fields =
            exact_fields(&mut lines, &["schema", "surface", "build_id", "stack", "payload_sha256"])
                .ok_or(WebMountError::Manifest("头字段缺失、重复或顺序不符"))?;
        let [schema, manifest_surface, manifest_build, manifest_stack, payload_sha256] = fields[..]
        else {
            return Err(WebMountError::Manifest("头字段数量不符"));
        };
        if schema != MANIFEST_SCHEMA {
            return Err(WebMountError::Manifest("schema 不符"));
        }
        if manifest_surface != surface || manifest_build != build_id {
            return Err(WebMountError::Manifest("surface 或 build_id 与所在目录不符"));
        }
        let policy = stack(manifest_stack).ok_or(WebMountError::Manifest("stack 不在闭集内"))?;
        if !is_hex_digest(payload_sha256) {
            return Err(WebMountError::Manifest("payload_sha256 不是 64 位小写十六进制"));
        }
        if lines.next() != Some("") {
            return Err(WebMountError::Manifest("头与文件行之间缺少空行"));
        }
        let mut files = Vec::new();
        for line in lines {
            files.push(parse_file_line(line)?);
        }
        Ok(Self { policy, payload_sha256: payload_sha256.to_owned(), files })
    }
}

/// `<sha256>  <./相对路径>  <identity|br-entity>`，列间恰为两个空格。
fn parse_file_line(line: &str) -> Result<ManifestFile, WebMountError> {
    let mut columns = line.split("  ");
    let (Some(sha256), Some(path), Some(kind), None) =
        (columns.next(), columns.next(), columns.next(), columns.next())
    else {
        return Err(WebMountError::Manifest("文件行必须是三列"));
    };
    if !is_hex_digest(sha256) {
        return Err(WebMountError::Manifest("文件摘要不是 64 位小写十六进制"));
    }
    let brotli_entity = match kind {
        "identity" => false,
        "br-entity" => true,
        _ => return Err(WebMountError::Manifest("文件表示不在闭集内")),
    };
    let relative =
        path.strip_prefix("./").ok_or_else(|| WebMountError::UnsafePath(path.to_owned()))?;
    if brotli_entity != relative.ends_with(".br") {
        return Err(WebMountError::Manifest("br-entity 必须且只能用于 .br 路径"));
    }
    Ok(ManifestFile { path: relative.to_owned(), sha256: sha256.to_owned(), brotli_entity })
}
