//! 新版正式包的页面导出树 `web-assets/<surface>/`：按摘要清单逐文件读入，供安装成只读 build。

use std::path::Path;

use super::install::{InstallFile, NewBuild};
use super::layout::{MAX_FILE_BYTES, MAX_TOTAL_BYTES, Stack, Surface};
use super::payload::PayloadList;
use super::verify::verify_file;
use crate::support::files;

/// 读取并核对导出树：清单是全集，每个文件的字节必须等于清单摘要，清单摘要必须等于包凭据里的
/// `payload_sha256`。包本身的来源与成员校验由 `deploy::artifact::inspect` 先行完成，这里不信任
/// 导出树在那之后没有变化，所以仍逐个重算。
pub(super) fn read_export_tree(
    root: &Path,
    surface: Surface,
    list: &PayloadList,
    build_id: &str,
    expected_payload_sha256: &str,
) -> anyhow::Result<NewBuild> {
    if list.payload_sha256 != expected_payload_sha256 {
        anyhow::bail!("{} 的摘要清单与包凭据的 payload_sha256 不一致", surface.as_str());
    }
    let tree = root.join("web-assets").join(surface.as_str());
    let mut total = 0_u64;
    let mut installed = Vec::with_capacity(list.files.len());
    for entry in &list.files {
        let bytes = files::read_bounded(&tree.join(&entry.path), MAX_FILE_BYTES)?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL_BYTES {
            anyhow::bail!("导出树总字节超过上限：{}", surface.as_str());
        }
        verify_file(&entry.path, &entry.sha256, &bytes)?;
        installed.push(InstallFile {
            path: entry.path.clone(),
            sha256: entry.sha256.clone(),
            brotli_entity: entry.path.ends_with(".br"),
            bytes,
        });
    }
    Ok(NewBuild {
        surface,
        build_id: build_id.to_owned(),
        stack: Stack::Solid,
        payload_sha256: list.payload_sha256.clone(),
        files: installed,
    })
}
