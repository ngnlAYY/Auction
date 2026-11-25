//! 新格式发布包的页面凭据：解析 `docs/WEB-STACK.v1`，核对版本硬边界、栈与 profile 的配对、
//! 两端摘要与页面导出树 `web-assets/<surface>/` 的路径集合和逐文件字节。全部检查离线进行，
//! 与 `ci/release/web-stack.sh` 的 `web_stack_verify_package` 同口径；脚本与 CLI 各自独立核对。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Context;

use super::{MAX_MANIFEST_BYTES, WebSurface};
use crate::support::digest::is_lower_hex;
use crate::support::files;

/// 首个含 `docs/WEB-STACK.v1` 的正式版本：不小于该版本的包必须带页面凭据，更小的 Rust 包按旧格式通过。
/// 与 `ci/release/verify-deployment-package.sh` 的 `first_web_stack_version` 同值，版本提升提交里一起写死。
pub(crate) const FIRST_WEB_STACK_VERSION: &str = "0.1.9";

const WEB_STACK_FILE: &str = "docs/WEB-STACK.v1";
const WEB_STACK_SCHEMA: &str = "auction-web-stack/1";
const WEB_ASSETS_PREFIX: &str = "web-assets/";
/// `WEB-STACK.v1` 的字段闭集，顺序固定。
const FIELDS: [&str; 8] = [
    "schema",
    "source_commit",
    "source_fingerprint",
    "public_stack",
    "admin_stack",
    "public_payload_sha256",
    "admin_payload_sha256",
    "build_id",
];

/// 页面技术栈；`WEB-STACK.v1` 的格式允许表达两种，校验只放行双 Solid。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stack {
    Dioxus,
    Solid,
}

impl Stack {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "dioxus" => Ok(Self::Dioxus),
            "solid" => Ok(Self::Solid),
            _ => anyhow::bail!("WEB-STACK.v1 的栈取值无效：{value}"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dioxus => "dioxus",
            Self::Solid => "solid",
        }
    }

    /// 该栈在 `WEB-<SURFACE>-MANIFEST.v1` 里唯一合法的 profile。
    pub(super) fn profile(self) -> &'static str {
        match self {
            Self::Dioxus => "wasm-release",
            Self::Solid => "solid-release",
        }
    }
}

/// 通过核对的页面凭据；来源提交与指纹已与 `RELEASE-MANIFEST.v1` 比对，不重复保存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WebStackIdentity {
    pub(crate) public_stack: Stack,
    pub(crate) admin_stack: Stack,
    pub(crate) public_payload_sha256: String,
    pub(crate) admin_payload_sha256: String,
    pub(crate) build_id: String,
}

impl WebStackIdentity {
    /// 与验证回执 `web_stack_id` 同形的标识。
    pub(crate) fn id(&self) -> String {
        format!("public-{}_admin-{}", self.public_stack.as_str(), self.admin_stack.as_str())
    }

    pub(crate) fn projection(&self) -> serde_json::Value {
        serde_json::json!({
            "web_stack_id": self.id(),
            "build_id": self.build_id,
        })
    }
}

/// 点分数字版本；任何非数字分量都失败关闭，边界判定不猜测。
fn parse_version(text: &str) -> anyhow::Result<Vec<u64>> {
    text.split('.')
        .map(|part| {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                anyhow::bail!("包版本不是点分数字：{text}");
            }
            part.parse::<u64>().with_context(|| format!("包版本分量过大：{text}"))
        })
        .collect()
}

/// `version` 是否不小于 `first`；较短的版本按补零比较。
fn version_at_least(version: &str, first: &str) -> anyhow::Result<bool> {
    let (mut left, mut right) = (parse_version(version)?, parse_version(first)?);
    let width = left.len().max(right.len());
    left.resize(width, 0);
    right.resize(width, 0);
    Ok(left >= right)
}

/// 从 `auction-rust-<版本>-<提交前12位>` 取包版本。
pub(super) fn package_version(package: &str) -> anyhow::Result<&str> {
    let rest = package
        .strip_prefix("auction-rust-")
        .with_context(|| format!("包名缺少 auction-rust- 前缀：{package}"))?;
    let (version, short) =
        rest.rsplit_once('-').with_context(|| format!("包名缺少提交后缀：{package}"))?;
    if !is_lower_hex(short, 12) {
        anyhow::bail!("包名的提交后缀无效：{package}");
    }
    parse_version(version)?;
    Ok(version)
}

/// 页面标记与摘要清单所用 profile：新格式按栈，旧格式固定 Rust 版。
pub(super) fn surface_profile(identity: Option<&WebStackIdentity>, surface: &str) -> Stack {
    match (identity, surface) {
        (Some(identity), "public") => identity.public_stack,
        (Some(identity), _) => identity.admin_stack,
        (None, _) => Stack::Dioxus,
    }
}

/// 读取并核对页面凭据的第一阶段：版本边界、字段与来源。栈与摘要的配对在 [`WebStackIdentity::verify_surfaces`]。
///
/// 版本不小于边界的包缺 `WEB-STACK.v1` 即拒；旧格式包不得携带导出树，混入 `web-assets/` 同样拒。
pub(super) fn read_identity(
    root: &Path,
    package: &str,
    source_commit: &str,
    source_fingerprint: &str,
    members: &BTreeMap<String, u32>,
    first_version: &str,
) -> anyhow::Result<Option<WebStackIdentity>> {
    let required = version_at_least(package_version(package)?, first_version)?;
    let Some(mode) = members.get(WEB_STACK_FILE) else {
        if required {
            anyhow::bail!("版本不小于 {first_version} 的发布包缺少 {WEB_STACK_FILE}");
        }
        if members.keys().any(|path| path.starts_with(WEB_ASSETS_PREFIX)) {
            anyhow::bail!("旧格式发布包不得携带页面导出树 web-assets/");
        }
        return Ok(None);
    };
    if mode & 0o777 != 0o644 {
        anyhow::bail!("{WEB_STACK_FILE} 必须为 0644");
    }
    let bytes = files::read_bounded(&root.join(WEB_STACK_FILE), MAX_MANIFEST_BYTES)?;
    let fields = parse_fields(&bytes)?;
    if fields["source_commit"] != source_commit
        || fields["source_fingerprint"] != source_fingerprint
    {
        anyhow::bail!("WEB-STACK.v1 的来源与 RELEASE-MANIFEST.v1 不一致");
    }
    let public_stack = Stack::parse(&fields["public_stack"])?;
    let admin_stack = Stack::parse(&fields["admin_stack"])?;
    if public_stack != admin_stack {
        anyhow::bail!(
            "发布包页面栈混用：public={} admin={}",
            public_stack.as_str(),
            admin_stack.as_str()
        );
    }
    if public_stack != Stack::Solid {
        anyhow::bail!("正式发布包的 Public 与 Admin 必须均为 solid");
    }
    Ok(Some(WebStackIdentity {
        public_stack,
        admin_stack,
        public_payload_sha256: fields["public_payload_sha256"].clone(),
        admin_payload_sha256: fields["admin_payload_sha256"].clone(),
        build_id: fields["build_id"].clone(),
    }))
}

/// 恰为八行、字段按固定顺序、LF 结尾、无空行与多余空白，值各自合乎形状。
fn parse_fields(bytes: &[u8]) -> anyhow::Result<BTreeMap<&'static str, String>> {
    let text = std::str::from_utf8(bytes).context("WEB-STACK.v1 不是 UTF-8")?;
    let body = text.strip_suffix('\n').context("WEB-STACK.v1 必须以 LF 结尾")?;
    let lines: Vec<&str> = body.split('\n').collect();
    if lines.len() != FIELDS.len() {
        anyhow::bail!("WEB-STACK.v1 必须恰为 {} 行", FIELDS.len());
    }
    let mut fields = BTreeMap::new();
    for (index, (line, key)) in lines.iter().zip(FIELDS).enumerate() {
        let value = line
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix('='))
            .with_context(|| format!("WEB-STACK.v1 第 {} 行应为 {key}", index + 1))?;
        let valid = match key {
            "schema" => value == WEB_STACK_SCHEMA,
            "source_commit" => is_lower_hex(value, 40),
            "public_stack" | "admin_stack" => matches!(value, "solid" | "dioxus"),
            "build_id" => {
                (1..=64).contains(&value.len())
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            }
            _ => is_lower_hex(value, 64),
        };
        if !valid {
            anyhow::bail!("WEB-STACK.v1 的 {key} 取值无效");
        }
        fields.insert(key, value.to_owned());
    }
    Ok(fields)
}

impl WebStackIdentity {
    /// 第二阶段：两端摘要等于 `docs/WEB-*-SHA256` 的原字节摘要，且导出树与摘要清单完全一致。
    ///
    /// 导出树的字节由调用方已核对过的 `SHA256SUMS` 担保（每个成员的摘要都与实际字节比对过），
    /// 这里只需要把成员摘要与页面摘要清单逐文件对上；`web-assets/` 下只允许 public 与 admin 两棵树。
    pub(super) fn verify_surfaces(
        &self,
        public: &WebSurface,
        admin: &WebSurface,
        members: &BTreeMap<String, u32>,
        checksums: &BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for (surface, declared, actual) in [
            ("public", &self.public_payload_sha256, public),
            ("admin", &self.admin_payload_sha256, admin),
        ] {
            if *declared != actual.payload_sha256 {
                anyhow::bail!(
                    "WEB-STACK.v1 的 {surface} 摘要与 WEB-{}-SHA256 不一致",
                    surface.to_uppercase()
                );
            }
            verify_tree(surface, actual, members, checksums)?;
        }
        let stray = members.keys().find(|path| {
            path.strip_prefix(WEB_ASSETS_PREFIX)
                .is_some_and(|rest| !rest.starts_with("public/") && !rest.starts_with("admin/"))
        });
        if let Some(path) = stray {
            anyhow::bail!("web-assets 只允许 public 与 admin 两棵导出树：{path}");
        }
        Ok(())
    }
}

/// 导出树的路径集合必须等于摘要清单，逐文件摘要相等，权限 0644。缺文件、多文件、同名异字节、
/// 其他 surface 的文件混入，都在此被拒。
fn verify_tree(
    surface: &str,
    web: &WebSurface,
    members: &BTreeMap<String, u32>,
    checksums: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let prefix = format!("{WEB_ASSETS_PREFIX}{surface}/");
    let actual: BTreeSet<&str> =
        members.keys().filter_map(|path| path.strip_prefix(prefix.as_str())).collect();
    let expected: BTreeSet<&str> = web.assets.keys().map(String::as_str).collect();
    if actual != expected {
        let missing = expected.difference(&actual).next();
        let extra = actual.difference(&expected).next();
        anyhow::bail!(
            "{surface} 导出树与摘要清单的文件集合不一致（缺少 {missing:?}，多出 {extra:?}）"
        );
    }
    for (path, digest) in &web.assets {
        let member = format!("{prefix}{path}");
        if members.get(&member).is_some_and(|mode| mode & 0o777 != 0o644) {
            anyhow::bail!("导出文件必须为 0644：{member}");
        }
        if checksums.get(&member) != Some(digest) {
            anyhow::bail!("{surface} 导出文件字节与摘要清单不一致：{member}");
        }
    }
    Ok(())
}
