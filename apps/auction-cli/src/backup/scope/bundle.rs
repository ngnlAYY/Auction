//! 备份范围打包格式：逐条记录把元数据与内容摘要绑定，读取时必须校验到显式结束标记。
//!
//! 布局（全部 UTF-8 JSON 行，除内容字节外）：
//! ```text
//! {"format":"auction-backup-scope-bundle.v1"}\n
//! {"kind":"runtime_config","path":"runtime_config/worker.toml","bytes":123}\n
//! <123 个内容字节>
//! {"sha256":"<sha256(头部行 + 内容)>"}\n
//! ...
//! {"end":true,"files":1,"format":"auction-backup-scope-bundle.v1"}\n
//! ```

use std::io::{BufRead, BufReader, Read, Write};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::collect::{
    Artifact, MAX_FILE_BYTES, MAX_RELATIVE_BYTES, SourceKind, open_verified, validate_relative,
};

pub(crate) const FORMAT: &str = "auction-backup-scope-bundle.v1";
const MAX_HEADER_BYTES: usize = 4096;
const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EntryDigest {
    pub(crate) kind: SourceKind,
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

pub(crate) struct WriteDigest {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) entries: Vec<EntryDigest>,
}

pub(crate) struct ReadDigest {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) entries: Vec<EntryDigest>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartHeader {
    format: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordHeader {
    kind: SourceKind,
    path: String,
    bytes: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordTrailer {
    sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndHeader {
    end: bool,
    files: u64,
    format: String,
}

/// 顺序写出全部文件；打包前重新核对每个文件的身份与修改时间。
pub(crate) fn write<W: Write>(inner: W, artifacts: &[Artifact]) -> anyhow::Result<WriteDigest> {
    let mut out = HashingWriter { inner, hasher: Sha256::new(), bytes: 0 };
    write_line(&mut out, &StartHeader { format: FORMAT.to_owned() })?;
    let mut entries = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let header = serde_json::to_vec(&RecordHeader {
            kind: artifact.kind,
            path: artifact.path.clone(),
            bytes: artifact.bytes,
        })
        .context("序列化范围包头失败")?;
        out.write_all(&header).context("写入范围包头失败")?;
        out.write_all(b"\n").context("写入范围包头失败")?;
        let mut hasher = Sha256::new();
        hasher.update(&header);
        hasher.update(b"\n");
        let mut file = open_verified(artifact)?;
        let mut remaining = artifact.bytes;
        let mut buffer = vec![0_u8; CHUNK];
        while remaining > 0 {
            let want = usize::try_from(remaining.min(CHUNK as u64)).unwrap_or(CHUNK);
            file.read_exact(&mut buffer[..want]).context("读取范围文件失败")?;
            hasher.update(&buffer[..want]);
            out.write_all(&buffer[..want]).context("写入范围内容失败")?;
            remaining -= want as u64;
        }
        if file.read(&mut buffer[..1]).context("读取范围文件失败")? != 0 {
            anyhow::bail!("备份范围文件在读取期间增长：{}", artifact.path);
        }
        let sha256 = auction_application::core::lower_hex(&hasher.finalize());
        write_line(&mut out, &RecordTrailer { sha256: sha256.clone() })?;
        entries.push(EntryDigest {
            kind: artifact.kind,
            path: artifact.path.clone(),
            bytes: artifact.bytes,
            sha256,
        });
    }
    write_line(
        &mut out,
        &EndHeader { end: true, files: entries.len() as u64, format: FORMAT.to_owned() },
    )?;
    let (sha256, bytes) = out.finish()?;
    Ok(WriteDigest { sha256, bytes, entries })
}

/// 读取并验证整个打包文件；内容以分块回调交给调用方，不在这里落盘。
pub(crate) fn read<R: Read, F>(inner: R, mut sink: F) -> anyhow::Result<ReadDigest>
where
    F: FnMut(&SourceKind, &str, &[u8]) -> anyhow::Result<()>,
{
    let mut reader = BufReader::new(HashingReader { inner, hasher: Sha256::new(), bytes: 0 });
    let start: StartHeader = parse_line(&mut reader, "范围包头意外结束")?;
    if start.format != FORMAT {
        anyhow::bail!("备份范围打包格式无效");
    }
    let mut entries: Vec<EntryDigest> = Vec::new();
    loop {
        let line = next_line(&mut reader)?.context("备份范围打包文件缺少结束标记")?;
        if let Ok(end) = serde_json::from_slice::<EndHeader>(&line) {
            if !end.end || end.format != FORMAT || end.files != entries.len() as u64 {
                anyhow::bail!("备份范围结束标记无效");
            }
            break;
        }
        let header: RecordHeader =
            serde_json::from_slice(&line).context("备份范围打包文件包含无效记录头")?;
        validate_record(&header, &entries)?;
        // 空分块作为条目开始标记，使零字节文件也会被创建并计入。
        sink(&header.kind, &header.path, &[])?;
        let mut hasher = Sha256::new();
        hasher.update(&line);
        let mut remaining = header.bytes;
        let mut buffer = vec![0_u8; CHUNK];
        while remaining > 0 {
            let want = usize::try_from(remaining.min(CHUNK as u64)).unwrap_or(CHUNK);
            reader.read_exact(&mut buffer[..want]).context("备份范围内容被截断")?;
            hasher.update(&buffer[..want]);
            sink(&header.kind, &header.path, &buffer[..want])?;
            remaining -= want as u64;
        }
        let trailer: RecordTrailer = parse_line(&mut reader, "备份范围记录缺少摘要")?;
        let sha256 = auction_application::core::lower_hex(&hasher.finalize());
        if trailer.sha256 != sha256 {
            anyhow::bail!("备份范围记录摘要不一致：{}", header.path);
        }
        entries.push(EntryDigest {
            kind: header.kind,
            path: header.path,
            bytes: header.bytes,
            sha256,
        });
    }
    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing).context("读取备份范围结尾失败")? != 0 {
        anyhow::bail!("备份范围结束标记后存在多余字节");
    }
    let hashing = reader.into_inner();
    let bytes = hashing.bytes;
    Ok(ReadDigest {
        sha256: auction_application::core::lower_hex(&hashing.hasher.finalize()),
        bytes,
        entries,
    })
}

fn validate_record(header: &RecordHeader, entries: &[EntryDigest]) -> anyhow::Result<()> {
    if header.bytes > MAX_FILE_BYTES || header.path.len() > MAX_RELATIVE_BYTES {
        anyhow::bail!("备份范围记录超出大小上限");
    }
    validate_relative(&header.path)?;
    let slug = header.kind.slug();
    let Some(rest) = header.path.strip_prefix(slug).and_then(|path| path.strip_prefix('/')) else {
        anyhow::bail!("备份范围记录路径与来源类型不一致");
    };
    if rest.is_empty() {
        anyhow::bail!("备份范围记录路径缺少文件名");
    }
    if entries.iter().any(|entry| entry.path == header.path) {
        anyhow::bail!("备份范围记录路径重复：{}", header.path);
    }
    Ok(())
}

fn parse_line<T: for<'de> Deserialize<'de>, R: BufRead>(
    reader: &mut R,
    missing: &'static str,
) -> anyhow::Result<T> {
    let line = next_line(reader)?.context(missing)?;
    serde_json::from_slice(&line).context("备份范围打包文件包含无效 JSON")
}

fn next_line<R: BufRead>(reader: &mut R) -> anyhow::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().context("读取备份范围打包文件失败")?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            anyhow::bail!("备份范围包头未以换行结束");
        }
        let (chunk, done) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (&available[..=index], true),
            None => (available, false),
        };
        if line.len() + chunk.len() > MAX_HEADER_BYTES {
            anyhow::bail!("备份范围包头超过 {MAX_HEADER_BYTES} 字节");
        }
        line.extend_from_slice(chunk);
        let used = chunk.len();
        reader.consume(used);
        if done {
            return Ok(Some(line));
        }
    }
}

fn write_line<W: Write, T: Serialize>(writer: &mut W, value: &T) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value).context("序列化范围包头失败")?;
    if bytes.len() > MAX_HEADER_BYTES {
        anyhow::bail!("备份范围包头超过 {MAX_HEADER_BYTES} 字节");
    }
    writer.write_all(&bytes).context("写入范围包头失败")?;
    writer.write_all(b"\n").context("写入范围包头失败")
}

struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

impl<W> HashingWriter<W> {
    fn finish(mut self) -> anyhow::Result<(String, u64)>
    where
        W: Write,
    {
        self.inner.flush().context("同步范围打包文件失败")?;
        Ok((auction_application::core::lower_hex(&self.hasher.finalize()), self.bytes))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.hasher.update(&bytes[..count]);
        self.bytes = self.bytes.saturating_add(count as u64);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        path::PathBuf,
    };

    use sha2::Digest;

    use super::{FORMAT, read, write};
    use crate::backup::scope::collect::{Plan, SourceSpec, collect};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("auction-scope-{label}-{}", uuid::Uuid::new_v4()));
            fs::DirBuilder::new().recursive(true).mode(0o700).create(&path).expect("目录");
            Self(path)
        }

        fn file(&self, name: &str, content: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                fs::DirBuilder::new().recursive(true).mode(0o700).create(parent).expect("子目录");
            }
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .expect("文件");
            std::io::Write::write_all(&mut file, content).expect("写入");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn plan(root: PathBuf) -> Plan {
        Plan {
            sources: vec![SourceSpec::Directory {
                kind: crate::backup::scope::collect::SourceKind::DeploymentReceipts,
                root,
                missing_ok: false,
            }],
            max_files: 16,
            maximum_total_bytes: 1024 * 1024,
        }
    }

    fn bundle(root: &TempTree) -> Vec<u8> {
        let collected = collect(&plan(root.0.clone())).expect("收集");
        let mut bytes = Vec::new();
        write(&mut bytes, &collected.artifacts).expect("打包");
        bytes
    }

    #[test]
    fn roundtrip_preserves_entries_and_digests() {
        let tree = TempTree::new("roundtrip");
        tree.file("b.json", b"{\"second\":true}");
        tree.file("nested/a.json", b"{\"first\":true}");
        let bytes = bundle(&tree);
        let mut extracted = Vec::new();
        let digest = read(Cursor::new(&bytes), |kind, path, chunk| {
            if !chunk.is_empty() {
                extracted.push((format!("{}:{path}", kind.slug()), chunk.to_vec()));
            }
            Ok(())
        })
        .expect("读取");
        assert_eq!(digest.bytes as usize, bytes.len());
        assert_eq!(digest.entries.len(), 2);
        let paths: Vec<_> = extracted.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "deployment_receipts:deployment_receipts/b.json",
                "deployment_receipts:deployment_receipts/nested/a.json"
            ]
        );
        let rebuilt: Vec<u8> =
            extracted.iter().flat_map(|(_, bytes)| bytes.iter().copied()).collect();
        assert_eq!(rebuilt, b"{\"second\":true}{\"first\":true}");
        assert_eq!(
            auction_application::core::lower_hex(&sha2::Sha256::digest(&bytes)),
            digest.sha256
        );
    }

    #[test]
    fn tampered_content_is_rejected() {
        let tree = TempTree::new("tamper");
        tree.file("x.json", b"secret-value");
        let mut bytes = bundle(&tree);
        let index =
            bytes.windows(b"secret-value".len()).position(|w| w == b"secret-value").unwrap();
        bytes[index] = b'S';
        assert!(read(Cursor::new(&bytes), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn tampered_header_is_rejected() {
        let tree = TempTree::new("header");
        tree.file("x.json", b"abc");
        let bytes = bundle(&tree);
        let text = String::from_utf8(bytes.clone()).unwrap();
        let patched = text.replace("x.json", "y.json");
        assert!(read(Cursor::new(patched.into_bytes()), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn truncated_and_trailing_bundles_are_rejected() {
        let tree = TempTree::new("truncate");
        tree.file("x.json", b"abc");
        let bytes = bundle(&tree);
        for broken in [&bytes[..bytes.len() - 4], bytes.as_slice()] {
            let mut input = broken.to_vec();
            input.push(b'x');
            assert!(read(Cursor::new(input), |_, _, _| Ok(())).is_err());
        }
        assert!(read(Cursor::new(&bytes[..bytes.len() - 8]), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn unknown_format_is_rejected() {
        let tree = TempTree::new("format");
        tree.file("x.json", b"abc");
        let bytes = bundle(&tree);
        let text =
            String::from_utf8(bytes).unwrap().replace(FORMAT, "auction-backup-scope-bundle.v2");
        assert!(read(Cursor::new(text.into_bytes()), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn collection_is_deterministic_and_rejects_symlinks() {
        let tree = TempTree::new("collect");
        tree.file("z.json", b"z");
        tree.file("a.json", b"a");
        let first = collect(&plan(tree.0.clone())).expect("收集");
        let second = collect(&plan(tree.0.clone())).expect("收集");
        let names: Vec<_> = first.artifacts.iter().map(|artifact| artifact.path.as_str()).collect();
        assert_eq!(names, vec!["deployment_receipts/a.json", "deployment_receipts/z.json"]);
        assert_eq!(
            first.artifacts.iter().map(|a| a.identity).collect::<Vec<_>>(),
            second.artifacts.iter().map(|a| a.identity).collect::<Vec<_>>()
        );
        std::os::unix::fs::symlink(tree.0.join("a.json"), tree.0.join("link.json")).unwrap();
        assert!(collect(&plan(tree.0.clone())).is_err());
    }
}
