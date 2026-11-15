//! SHA-256 摘要与十六进制判定：备份、部署、回执与运维证据共用的唯一实现。

use std::{fs::File, io::Read, path::Path};

use anyhow::Context;
use auction_application::core::lower_hex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::support::files;

/// 流式读取的缓冲区大小。
const BUFFER_BYTES: usize = 256 * 1024;

/// 字节串的小写十六进制 SHA-256。
pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    lower_hex(&Sha256::digest(bytes))
}

/// 读到 EOF 为止，返回摘要与实际读取的字节数；字节数与摘要来自同一次读取。
pub(crate) fn sha256_reader(mut reader: impl Read) -> std::io::Result<(String, u64)> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((lower_hex(&digest.finalize()), total))
}

/// 打开路径并计算摘要与字节数；跟随符号链接（`/proc/<pid>/exe` 依赖这一点），文件类型由
/// 调用方先行核对。
pub(crate) fn sha256_path(path: &Path) -> anyhow::Result<(String, u64)> {
    let file = File::open(path).with_context(|| format!("打开文件失败：{}", path.display()))?;
    sha256_reader(file).with_context(|| format!("读取文件失败：{}", path.display()))
}

/// 异步流式计算普通文件摘要；拒绝符号链接与特殊文件。
pub(crate) async fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::from_std(files::open_regular(path)?);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("读取文件失败：{}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(lower_hex(&digest.finalize()))
}

/// 恰好 `length` 位的小写十六进制（`0-9a-f`）；大写与其他字符一律拒绝。
pub(crate) fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{is_lower_hex, sha256_bytes, sha256_reader};

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn digests_agree_and_count_bytes() {
        assert_eq!(sha256_bytes(b""), EMPTY_SHA256);
        let (digest, bytes) = sha256_reader(Cursor::new(b"auction".to_vec())).expect("读取");
        assert_eq!(digest, sha256_bytes(b"auction"));
        assert_eq!(bytes, 7);
        // 跨越缓冲区边界的输入与一次性摘要一致。
        let large = vec![7_u8; super::BUFFER_BYTES * 2 + 3];
        let (digest, bytes) = sha256_reader(Cursor::new(large.clone())).expect("读取");
        assert_eq!(digest, sha256_bytes(&large));
        assert_eq!(bytes, large.len() as u64);
    }

    #[test]
    fn lower_hex_requires_exact_length_and_lowercase() {
        assert!(is_lower_hex(EMPTY_SHA256, 64));
        assert!(is_lower_hex(&"0".repeat(40), 40));
        assert!(!is_lower_hex(&EMPTY_SHA256.to_uppercase(), 64));
        assert!(!is_lower_hex(&EMPTY_SHA256[..63], 64));
        assert!(!is_lower_hex(&format!("{EMPTY_SHA256}0"), 64));
        assert!(!is_lower_hex(&"g".repeat(64), 64));
        assert!(!is_lower_hex("", 64));
        assert!(is_lower_hex("", 0));
    }
}
