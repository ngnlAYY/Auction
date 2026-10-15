//! 仓库文档静态门禁：链接、锚点、导航覆盖与包 README，不执行产品行为。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

fn main() {
    if let Err(error) = check() {
        eprintln!("文档检查失败：{error}");
        std::process::exit(1);
    }
}

fn check() -> Result<(), String> {
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    let paths = scan::documents(&root)?;
    let mut documents = BTreeMap::new();
    for path in &paths {
        let content =
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        documents.insert(path.clone(), scan::parse(path, &content)?);
    }
    let mut graph = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
    for (path, document) in &documents {
        for link in &document.links {
            if let Some(target) = links::resolve(&root, path, link, &documents)? {
                graph.entry(path.clone()).or_default().insert(target);
            }
        }
    }
    let mut visited = BTreeSet::new();
    let mut pending = vec![root.join("README.md")];
    while let Some(path) = pending.pop() {
        if visited.insert(path.clone()) {
            pending.extend(graph.get(&path).into_iter().flatten().cloned());
        }
    }
    let unreachable: Vec<_> = paths.difference(&visited).collect();
    if !unreachable.is_empty() {
        return Err(format!(
            "下列正式文档无法从根 README 到达，请更新文档导航：{}",
            unreachable
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    scan::package_readmes(&root)?;
    println!("文档检查通过：{} 份文档；本地链接、锚点、导航覆盖和包 README 有效。", paths.len());
    Ok(())
}

mod scan {
    //! 正式文档范围及仓库采用的 Markdown 子集扫描。

    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::{Path, PathBuf},
    };

    /// Markdown 中的本地或外部链接及错误定位行号。
    pub(super) struct Link {
        pub line: usize,
        pub target: String,
    }

    /// 文档的标题锚点与出链，供结构和可达性检查使用。
    pub(super) struct Document {
        pub anchors: BTreeSet<String>,
        pub links: Vec<Link>,
    }

    /// 收集正式入口与约定目录，拒绝符号链接绕过仓库边界。
    pub(super) fn documents(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
        let mut paths = BTreeSet::new();
        for entry in ["README.md", "AGENTS.md"] {
            let path = root.join(entry);
            if !path.is_file() {
                return Err(format!("缺少正式入口：{entry}"));
            }
            paths.insert(path);
        }
        for directory in ["docs", "apps", "crates", "ops", "migrations", "ci"] {
            collect(&root.join(directory), &mut paths)?;
        }
        Ok(paths)
    }

    fn collect(directory: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<(), String> {
        for entry in
            fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let path = entry.path();
            // 文档不能借符号链接跳出仓库，或把私有/生成目录纳入检查。
            if kind.is_symlink() {
                return Err(format!("正式文档目录中不接受符号链接：{}", path.display()));
            }
            if kind.is_dir() {
                collect(&path, paths)?;
            } else if path.extension().is_some_and(|extension| extension == "md") {
                paths.insert(path);
            }
        }
        Ok(())
    }

    /// 每个 Cargo 包必须随源码提供职责说明。
    pub(super) fn package_readmes(root: &Path) -> Result<(), String> {
        for directory in ["apps", "crates"] {
            for entry in fs::read_dir(root.join(directory)).map_err(|error| error.to_string())? {
                let path = entry.map_err(|error| error.to_string())?.path();
                if path.join("Cargo.toml").is_file() && !path.join("README.md").is_file() {
                    return Err(format!("Cargo 包缺少 README：{}", path.display()));
                }
            }
        }
        Ok(())
    }

    /// 解析仓库使用的 Markdown 子集，不解析代码围栏中的示例。
    pub(super) fn parse(path: &Path, content: &str) -> Result<Document, String> {
        let mut anchors = BTreeSet::new();
        let mut counts = BTreeMap::<String, usize>::new();
        let mut links = Vec::new();
        let mut fence: Option<(char, usize)> = None;
        if !content.starts_with("# ") {
            return Err(format!("{}: 第一行必须是一级标题", path.display()));
        }
        for (index, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            let marker = trimmed.chars().next().filter(|ch| matches!(ch, '`' | '~'));
            let width =
                marker.map_or(0, |ch| trimmed.chars().take_while(|value| *value == ch).count());
            if width >= 3 {
                let ch = marker.ok_or("无法解析代码围栏")?;
                match fence {
                    None => fence = Some((ch, width)),
                    Some((opening, size)) if opening == ch && width >= size => fence = None,
                    _ => {}
                }
                continue;
            }
            if fence.is_some() || line.starts_with("    ") {
                continue;
            }
            if let Some(heading) = trimmed.strip_prefix('#') {
                let heading = heading.trim_start_matches('#');
                if heading.starts_with(' ') {
                    let slug: String = heading
                        .trim()
                        .to_lowercase()
                        .chars()
                        .filter_map(|ch| {
                            if ch.is_alphanumeric() || matches!(ch, '_' | '-') {
                                Some(ch)
                            } else if ch == ' ' {
                                Some('-')
                            } else {
                                None
                            }
                        })
                        .collect();
                    let count = counts.entry(slug.clone()).or_default();
                    anchors.insert(if *count == 0 { slug } else { format!("{slug}-{count}") });
                    *count += 1;
                }
            }
            // 当前正式文档只采用行内链接，引用式语法必须先扩展检查器。
            if trimmed.starts_with('[') && trimmed.contains("]:") {
                return Err(format!("{}:{}: 请使用行内 Markdown 链接", path.display(), index + 1));
            }
            let mut remaining = line;
            while let Some(start) = remaining.find("](") {
                let destination = &remaining[start + 2..];
                let mut depth = 1_u32;
                let mut end = None;
                let mut escaped = false;
                for (offset, ch) in destination.char_indices() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    match ch {
                        '\\' => escaped = true,
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(offset);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let end =
                    end.ok_or_else(|| format!("{}:{}: 未闭合的链接", path.display(), index + 1))?;
                links.push(Link { line: index + 1, target: destination[..end].trim().to_owned() });
                remaining = &destination[end + 1..];
            }
        }
        if fence.is_some() {
            return Err(format!("{}: 代码围栏未闭合", path.display()));
        }
        Ok(Document { anchors, links })
    }
}

mod links {
    //! 本地链接及 ATX 标题锚点验证，不访问网络。

    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use crate::scan::{Document, Link};

    /// 解析并验证仓库内目标与锚点，外部链接不联网。
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
        let error = |message: &str| {
            format!("{}:{}: {message}：{}", source.display(), link.line, link.target)
        };
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
}
