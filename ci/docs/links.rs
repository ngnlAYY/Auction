//! 本地链接及 ATX 标题锚点验证，不访问网络。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::scan::{Document, Link};

pub(super) fn resolve(
    root: &Path,
    source: &Path,
    link: &Link,
    documents: &BTreeMap<PathBuf, Document>,
) -> Result<Option<PathBuf>, String> {
    let target = link.target.trim_start_matches('<').trim_end_matches('>');
    if target.starts_with("https://")
        || target.starts_with("http://")
        || target.starts_with("mailto:")
    {
        return Ok(None);
    }
    let error =
        |message: &str| format!("{}:{}: {message}：{}", source.display(), link.line, link.target);
    let (file, fragment) =
        target.split_once('#').map_or((target, None), |(file, id)| (file, Some(id)));
    let file = decode(file).map_err(|_| error("非法 URL 编码"))?;
    if Path::new(&file).is_absolute() || file.contains('?') || file.contains(':') {
        return Err(error("本地链接必须使用无查询的仓库相对路径"));
    }
    let mut resolved = if file.is_empty() {
        source.to_owned()
    } else {
        source.parent().ok_or_else(|| error("无父目录"))?.join(&file)
    };
    resolved = resolved.canonicalize().map_err(|_| error("链接目标不存在"))?;
    if !resolved.starts_with(root) {
        return Err(error("链接跳出仓库"));
    }
    if resolved.is_dir() && resolved.join("README.md").is_file() {
        resolved.push("README.md");
    }
    let document = documents.get(&resolved);
    if let Some(fragment) = fragment.filter(|fragment| !fragment.is_empty()) {
        let fragment = decode(fragment).map_err(|_| error("非法锚点编码"))?;
        if document.is_some_and(|document| !document.anchors.contains(&fragment)) {
            return Err(error("标题锚点不存在"));
        }
    }
    Ok(document.map(|_| resolved))
}

fn decode(value: &str) -> Result<String, ()> {
    let mut output = Vec::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = bytes.get(index + 1..index + 3).ok_or(())?;
            let digits = std::str::from_utf8(digits).map_err(|_| ())?;
            output.push(u8::from_str_radix(digits, 16).map_err(|_| ())?);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|_| ())
}
