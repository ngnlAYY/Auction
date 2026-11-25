//! 从旧二进制的回环源站按清单逐文件取回页面字节，并在落盘前逐个核对。
//!
//! 清单是全集：原文请求 `Accept-Encoding: identity`；`x.br` 从 `x` 请求 Brotli 表示并保存
//! 未解压的实体字节。取回过程只在内存里累积，没有任何文件写入；是否安装由调用方在全部通过后决定。

use auction_integrations::{WebFetchClient, WebRepresentation};

use super::install::InstallFile;
use super::layout::{INDEX_HTML, MAX_TOTAL_BYTES};
use super::payload::PayloadList;
use super::verify::{verify_file, verify_index_policy};

/// 取回并核对清单里的每个文件；任何一个不符立即失败，不返回部分结果。
pub(super) async fn fetch_all(
    client: &WebFetchClient,
    list: &PayloadList,
) -> anyhow::Result<Vec<InstallFile>> {
    let mut fetched = Vec::with_capacity(list.files.len());
    let mut total = 0_u64;
    for entry in &list.files {
        let (request_path, representation, brotli_entity) = match entry.path.strip_suffix(".br") {
            Some(base) => (base, WebRepresentation::Brotli, true),
            None => (entry.path.as_str(), WebRepresentation::Identity, false),
        };
        let response = client.get(request_path, representation).await?;
        total += response.body.len() as u64;
        if total > MAX_TOTAL_BYTES {
            anyhow::bail!("取回的总字节超过上限 {MAX_TOTAL_BYTES}");
        }
        verify_file(&entry.path, &entry.sha256, &response.body)?;
        if entry.path == INDEX_HTML {
            verify_index_policy(response.content_security_policy.as_deref())?;
        }
        fetched.push(InstallFile {
            path: entry.path.clone(),
            sha256: entry.sha256.clone(),
            brotli_entity,
            bytes: response.body,
        });
    }
    Ok(fetched)
}
