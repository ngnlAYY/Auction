//! 页面摘要清单 `WEB-<SURFACE>-SHA256`（`<sha256>  ./<相对路径>`）的解析：物化与导出树安装共用的
//! “全集”定义。`payload_sha256` 是清单**原字节**的 SHA-256。

use std::collections::BTreeSet;

use anyhow::Context;

use super::layout::{INDEX_HTML, MAX_FILES, is_hex_digest, validate_relative_path};
use crate::support::digest::sha256_bytes;

/// 清单里的一个文件及其期望摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PayloadFile {
    pub(super) path: String,
    pub(super) sha256: String,
}

/// 解析后的摘要清单：路径集合就是全集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PayloadList {
    pub(super) payload_sha256: String,
    pub(super) files: Vec<PayloadFile>,
}

impl PayloadList {
    pub(super) fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let text = std::str::from_utf8(bytes).context("页面摘要清单不是 UTF-8")?;
        let mut files = Vec::new();
        let mut seen = BTreeSet::new();
        for line in text.lines() {
            let (digest, path) = line.split_once("  ").context("页面摘要清单行格式无效")?;
            let relative = path.strip_prefix("./").context("页面摘要清单路径缺少 ./ 前缀")?;
            if !is_hex_digest(digest) {
                anyhow::bail!("页面摘要清单摘要无效：{relative}");
            }
            validate_relative_path(relative)?;
            if !seen.insert(relative.to_owned()) {
                anyhow::bail!("页面摘要清单重复记录：{relative}");
            }
            files.push(PayloadFile { path: relative.to_owned(), sha256: digest.to_owned() });
            if files.len() > MAX_FILES {
                anyhow::bail!("页面摘要清单文件数超过上限 {MAX_FILES}");
            }
        }
        if !seen.contains(INDEX_HTML) {
            anyhow::bail!("页面摘要清单缺少 {INDEX_HTML}");
        }
        // 预压缩副本只能附属于已登记的原文。
        if let Some(orphan) = seen
            .iter()
            .find(|path| path.strip_suffix(".br").is_some_and(|base| !seen.contains(base)))
        {
            anyhow::bail!("预压缩副本缺少对应原文：{orphan}");
        }
        Ok(Self { payload_sha256: sha256_bytes(bytes), files })
    }
}
