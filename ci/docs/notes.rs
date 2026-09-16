//! 活跃决策笔记的路径、状态和必备章节检查。

use std::{path::Path, process::Command};

pub(super) fn check(root: &Path, path: &Path, content: &str) -> Result<(), String> {
    let relative =
        path.strip_prefix(root.join(".agents/notes")).map_err(|error| error.to_string())?;
    let parts: Vec<_> = relative.iter().map(|part| part.to_string_lossy()).collect();
    let error = |message: &str| format!("{}: {message}", path.display());
    // 笔记被忽略会导致本地检查通过、提交后失去决策；只读检查，不自动暂存文件。
    let ignored = Command::new("git")
        .args(["check-ignore", "--no-index", "--quiet", "--"])
        .arg(path)
        .status()
        .map_err(|_| error("无法检查笔记的 Git 忽略规则"))?;
    if ignored.code() != Some(1) {
        return Err(error("笔记被忽略或 Git 规则检查失败，不能随变更交付"));
    }
    if parts.len() != 3 {
        return Err(error("笔记路径必须为 lifecycle/class/yyyy-mm-dd-topic.md"));
    }
    let lifecycle = parts[0].as_ref();
    if lifecycle == "archived" {
        return Err(error("当前没有归档封印工具；首次归档前须建立 manifest 哈希及只增不改检查"));
    }
    if !["proposed", "implemented", "rejected"].contains(&lifecycle) {
        return Err(error("未知笔记生命周期"));
    }
    if !["feature", "bug-fix", "simplification", "architecture", "process", "testing"]
        .contains(&parts[1].as_ref())
    {
        return Err(error("未知笔记分类"));
    }
    let filename = parts[2].as_bytes();
    if filename.len() < 15
        || filename[4] != b'-'
        || filename[7] != b'-'
        || filename[10] != b'-'
        || !filename[..10]
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
        || !parts[2].ends_with(".md")
    {
        return Err(error("笔记名必须为首次提出日与 topic"));
    }
    let lines: Vec<_> = content.lines().collect();
    let expected = format!("Status: {lifecycle}");
    if lines.len() < 5
        || !lines[0].starts_with("# Agent Note: ")
        || !lines[1].is_empty()
        || !lines[3].is_empty()
    {
        return Err(error("笔记头块必须为标题、空行、状态、空行"));
    }
    if (lifecycle != "rejected" && lines[2] != expected)
        || (lifecycle == "rejected" && !lines[2].starts_with("Status: rejected — "))
        || lines.iter().filter(|line| line.starts_with("Status:")).count() != 1
    {
        return Err(error("笔记状态与路径不一致，或 rejected 缺少原因"));
    }
    let sections: Vec<_> = lines.iter().filter_map(|line| line.strip_prefix("## ")).collect();
    if sections.first() != Some(&"Problem") {
        return Err(error("首节必须为 Problem"));
    }
    let required = if lifecycle == "implemented" {
        vec!["Problem", "Decision", "Alternatives considered", "Consequences"]
    } else {
        vec!["Problem", "Proposal", "Alternatives considered", "Acceptance criteria", "Risks"]
    };
    for section in required {
        if !sections.contains(&section) {
            return Err(error(&format!("缺少章节 {section}")));
        }
    }
    if lifecycle == "implemented"
        && sections.iter().any(|section| {
            [
                "Proposal",
                "Plan",
                "Migration plan",
                "Acceptance criteria",
                "Risks",
                "提案",
                "方案",
                "计划",
                "迁移计划",
                "验收标准",
                "风险",
            ]
            .contains(section)
        })
    {
        return Err(error("implemented 正文不得保留提案/计划章节"));
    }
    Ok(())
}
