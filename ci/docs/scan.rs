//! 正式文档范围及仓库采用的 Markdown 子集扫描。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Link {
    pub line: usize,
    pub target: String,
}

pub(super) struct Document {
    pub anchors: BTreeSet<String>,
    pub links: Vec<Link>,
}

pub(super) fn documents(root: &Path) -> Result<BTreeSet<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    for entry in ["README.md", "AGENTS.md", "DESIGN.md"] {
        let path = root.join(entry);
        if !path.is_file() {
            return Err(format!("缺少正式入口：{entry}"));
        }
        paths.insert(path);
    }
    for directory in ["docs", "apps", "crates", "ops", "migrations", "ci", ".agents/notes"] {
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
        let width = marker.map_or(0, |ch| trimmed.chars().take_while(|value| *value == ch).count());
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
