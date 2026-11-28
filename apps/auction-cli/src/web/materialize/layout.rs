//! 静态挂载的磁盘格式：`<web_root>/<surface>/<build_id>/{files/,MANIFEST.v1}` 与入口指针 `ENTRY.v1`。
//!
//! 文本格式由服务端挂载加载器（`auction-http` 的 `assets::mount`）读取并严格解析；这里是写入与
//! 自检读取的另一份实现，字段、顺序与语义必须与它逐字一致。两个文件都是 `key=value` 行，
//! 字段恰为且按固定顺序出现，LF 结尾，不做宽容解析。

use anyhow::Context;

pub(super) const ENTRY_FILE: &str = "ENTRY.v1";
pub(super) const MANIFEST_FILE: &str = "MANIFEST.v1";
pub(super) const FILES_DIR: &str = "files";
pub(super) const INDEX_HTML: &str = "index.html";
/// 与服务端挂载加载器及发布包成员上限同量级。
pub(super) const MAX_FILES: usize = 4096;
pub(super) const MAX_MANIFEST_BYTES: u64 = 4 << 20;
pub(super) const MAX_ENTRY_BYTES: u64 = 4096;
pub(super) const MAX_FILE_BYTES: u64 = 64 << 20;
pub(super) const MAX_TOTAL_BYTES: u64 = 512 << 20;

const ENTRY_SCHEMA: &str = "auction-web-entry/1";
const MANIFEST_SCHEMA: &str = "auction-web-build/1";

/// 页面 surface；两端目录互不引用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Surface {
    Public,
    Admin,
}

impl Surface {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Admin => "admin",
        }
    }

    /// 发布包 `docs/WEB-<SURFACE>-*` 文件名里的大写形式。
    pub(super) const fn upper(self) -> &'static str {
        match self {
            Self::Public => "PUBLIC",
            Self::Admin => "ADMIN",
        }
    }
}

/// 页面技术栈；入口的脚本策略由它推出，不是自由输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stack {
    Dioxus,
    Solid,
}

impl Stack {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Dioxus => "dioxus",
            Self::Solid => "solid",
        }
    }

    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "dioxus" => Ok(Self::Dioxus),
            "solid" => Ok(Self::Solid),
            _ => anyhow::bail!("stack 不在闭集内：{value}"),
        }
    }
}

pub(super) fn is_hex_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// 构建号同时是目录名：1–64 位、首字符为 ASCII 字母数字，其余 `[A-Za-z0-9._-]`。
/// 与服务端 `auction-observability::is_valid_build_id` 同一规则：`-x`、`_x` 之类能让
/// `web install/switch` 装好并切换到服务启动时拒绝的挂载，必须在这里就拒绝。
pub(super) fn validate_build_id(value: &str) -> anyhow::Result<()> {
    let valid = (1..=64).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
    if valid { Ok(()) } else { anyhow::bail!("build_id 含非法字符或长度：{value}") }
}

/// 清单相对路径：非空段、无 `.` 与 `..`、无反斜线与控制字符、不以 `/` 开头。
pub(super) fn validate_relative_path(path: &str) -> anyhow::Result<()> {
    let unsafe_path = path.is_empty()
        || path.len() > 4096
        || path.contains('\\')
        || path.bytes().any(|byte| byte.is_ascii_control())
        || path.split('/').any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
    if unsafe_path { anyhow::bail!("清单路径不安全：{path}") } else { Ok(()) }
}

/// 一个 build 的清单：路径集合就是全集，没有清单外文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BuildManifest {
    pub(super) surface: Surface,
    pub(super) build_id: String,
    pub(super) stack: Stack,
    pub(super) payload_sha256: String,
    pub(super) files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ManifestFile {
    pub(super) path: String,
    pub(super) sha256: String,
    /// `true` 表示该行是 `x.br`，保存的是 `Content-Encoding: br` 的原实体字节。
    pub(super) brotli_entity: bool,
}

impl BuildManifest {
    pub(super) fn render(&self) -> String {
        let mut text = format!(
            "schema={MANIFEST_SCHEMA}\nsurface={}\nbuild_id={}\nstack={}\npayload_sha256={}\n\n",
            self.surface.as_str(),
            self.build_id,
            self.stack.as_str(),
            self.payload_sha256
        );
        for file in &self.files {
            let kind = if file.brotli_entity { "br-entity" } else { "identity" };
            text.push_str(&format!("{}  ./{}  {kind}\n", file.sha256, file.path));
        }
        text
    }

    pub(super) fn parse(text: &str) -> anyhow::Result<Self> {
        let body = text.strip_suffix('\n').context("清单缺少结尾换行")?;
        let mut lines = body.split('\n');
        let fields =
            exact_fields(&mut lines, &["schema", "surface", "build_id", "stack", "payload_sha256"])
                .context("清单头字段缺失、重复或顺序不符")?;
        let [schema, surface, build_id, stack, payload_sha256] = fields[..] else {
            anyhow::bail!("清单头字段数量不符");
        };
        if schema != MANIFEST_SCHEMA {
            anyhow::bail!("清单 schema 不符");
        }
        let surface = match surface {
            "public" => Surface::Public,
            "admin" => Surface::Admin,
            _ => anyhow::bail!("清单 surface 不在闭集内"),
        };
        validate_build_id(build_id)?;
        if !is_hex_digest(payload_sha256) {
            anyhow::bail!("清单 payload_sha256 不是 64 位小写十六进制");
        }
        if lines.next() != Some("") {
            anyhow::bail!("清单头与文件行之间缺少空行");
        }
        let files = lines.map(parse_file_line).collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Self {
            surface,
            build_id: build_id.to_owned(),
            stack: Stack::parse(stack)?,
            payload_sha256: payload_sha256.to_owned(),
            files,
        })
    }
}

/// `<sha256>  <./相对路径>  <identity|br-entity>`，列间恰为两个空格。
fn parse_file_line(line: &str) -> anyhow::Result<ManifestFile> {
    let mut columns = line.split("  ");
    let (Some(sha256), Some(path), Some(kind), None) =
        (columns.next(), columns.next(), columns.next(), columns.next())
    else {
        anyhow::bail!("清单文件行必须是三列");
    };
    if !is_hex_digest(sha256) {
        anyhow::bail!("清单文件摘要不是 64 位小写十六进制");
    }
    let brotli_entity = match kind {
        "identity" => false,
        "br-entity" => true,
        _ => anyhow::bail!("清单文件表示不在闭集内"),
    };
    let relative =
        path.strip_prefix("./").with_context(|| format!("清单路径缺少 ./ 前缀：{path}"))?;
    validate_relative_path(relative)?;
    if brotli_entity != relative.ends_with(".br") {
        anyhow::bail!("br-entity 必须且只能用于 .br 路径：{relative}");
    }
    Ok(ManifestFile { path: relative.to_owned(), sha256: sha256.to_owned(), brotli_entity })
}

/// 一个 surface 的入口指针：默认入口 build 与独立入口 build，整体原子替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntryPointer {
    pub(super) surface: Surface,
    pub(super) build_id: String,
    pub(super) stack: Stack,
    pub(super) entry_sha256: String,
    pub(super) payload_sha256: String,
    pub(super) independent_build_id: String,
    pub(super) independent_payload_sha256: String,
}

impl EntryPointer {
    pub(super) fn render(&self) -> String {
        // script_policy 由 stack 推出；冗余写出供服务端互验，拒绝错配。
        format!(
            "schema={ENTRY_SCHEMA}\nsurface={}\nbuild_id={}\nstack={stack}\nscript_policy={stack}\n\
entry_sha256={}\npayload_sha256={}\nindependent_build_id={}\nindependent_payload_sha256={}\n",
            self.surface.as_str(),
            self.build_id,
            self.entry_sha256,
            self.payload_sha256,
            self.independent_build_id,
            self.independent_payload_sha256,
            stack = self.stack.as_str(),
        )
    }

    pub(super) fn parse(text: &str) -> anyhow::Result<Self> {
        let body = text.strip_suffix('\n').context("入口指针缺少结尾换行")?;
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
        .context("入口指针字段缺失、重复或顺序不符")?;
        if lines.next().is_some() {
            anyhow::bail!("入口指针存在多余行");
        }
        let [
            schema,
            surface,
            build_id,
            stack,
            script_policy,
            entry_sha256,
            payload_sha256,
            independent_build_id,
            independent_payload_sha256,
        ] = fields[..]
        else {
            anyhow::bail!("入口指针字段数量不符");
        };
        if schema != ENTRY_SCHEMA {
            anyhow::bail!("入口指针 schema 不符");
        }
        let surface = match surface {
            "public" => Surface::Public,
            "admin" => Surface::Admin,
            _ => anyhow::bail!("入口指针 surface 不在闭集内"),
        };
        let stack = Stack::parse(stack)?;
        if Stack::parse(script_policy)? != stack {
            anyhow::bail!("script_policy 与 stack 不一致");
        }
        validate_build_id(build_id)?;
        validate_build_id(independent_build_id)?;
        for digest in [entry_sha256, payload_sha256, independent_payload_sha256] {
            if !is_hex_digest(digest) {
                anyhow::bail!("入口指针摘要不是 64 位小写十六进制");
            }
        }
        Ok(Self {
            surface,
            build_id: build_id.to_owned(),
            stack,
            entry_sha256: entry_sha256.to_owned(),
            payload_sha256: payload_sha256.to_owned(),
            independent_build_id: independent_build_id.to_owned(),
            independent_payload_sha256: independent_payload_sha256.to_owned(),
        })
    }
}

/// 取出恰为给定键序列的值；行数、键名或顺序不符返回 `None`。
fn exact_fields<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    keys: &[&str],
) -> Option<Vec<&'a str>> {
    keys.iter().map(|key| lines.next()?.strip_prefix(key)?.strip_prefix('=')).collect()
}
