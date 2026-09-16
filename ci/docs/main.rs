//! 仓库文档静态门禁：链接、导航覆盖与决策笔记结构，不执行产品行为。

mod links;
mod notes;
mod scan;

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
        if path.starts_with(root.join(".agents/notes")) {
            notes::check(&root, path, &content)?;
        }
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
    println!(
        "文档检查通过：{} 份文档；本地链接、锚点、导航覆盖、包 README 和笔记结构有效。",
        paths.len()
    );
    Ok(())
}
