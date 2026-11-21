//! 静态核对审计失败标记、唯一的隔离登记及其生成的测试入口。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MARKER: &str = "// 审计失败注入：";
const RUNNER: &str = include_str!("mod.rs");

fn read_sources(directory: &Path, sources: &mut Vec<(PathBuf, String)>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("读取测试源码目录 {} 失败：{error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("读取测试源码目录条目失败：{error}"));
        let path = entry.path();
        let kind =
            entry.file_type().unwrap_or_else(|error| panic!("读取测试源码类型失败：{error}"));
        if kind.is_dir() {
            read_sources(&path, sources);
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            let source = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("读取测试源码 {} 失败：{error}", path.display()));
            sources.push((path, source));
        }
    }
}

fn identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn identifier(word: &str) -> bool {
    word.starts_with(|ch: char| ch.is_ascii_lowercase() || ch == '_')
        && word.bytes().all(identifier_byte)
}

fn compact(source: &str) -> String {
    source.split_whitespace().collect::<String>().replace(",)", ")")
}

fn check_isolation(
    tests: &[(PathBuf, String)],
    scenarios: &[(PathBuf, String)],
    runner: &str,
) -> Result<(), String> {
    let runner = compact(runner);
    let parent_entrypoint = compact(
        "#[tokio::test]
         async fn $parent() {
             $crate::consignment::audit_subprocess::run_audit_failure_test(
                 module_path!(), stringify!($child)
             ).await;
         }",
    );
    let child_entrypoint = compact(
        r#"#[tokio::test]
           #[ignore = "由父测试在子进程中运行：会写入进程级审计健康计数"]
           async fn $child() {
               $crate::consignment::audit_subprocess::run_audit_failure_scenario($scenario).await;
           }"#,
    );
    if !runner.contains(&parent_entrypoint)
        || !runner.contains(&child_entrypoint)
        || !runner.contains("scenario(&store,&maintenance).await")
    {
        return Err("隔离宏必须由父测试启动生成的忽略子测试，且子测试只执行登记的场景".into());
    }

    let test_sources: Vec<_> = tests.iter().map(|(path, source)| (path, compact(source))).collect();
    let mut parents = HashSet::new();
    let mut marked_scenarios = HashSet::new();
    for (owner, source) in tests.iter().chain(scenarios) {
        for marked in source.lines().filter_map(|line| line.trim_start().strip_prefix(MARKER)) {
            let words: Vec<_> = marked.split_whitespace().collect();
            let [parent, scenario] = words.as_slice() else {
                return Err(format!("{} 的标记必须登记父测试与场景函数名", owner.display()));
            };
            if !parent.starts_with("postgres_") || !identifier(parent) || !identifier(scenario) {
                return Err(format!("{} 的审计失败注入标记无效", owner.display()));
            }
            if !parents.insert(*parent) || !marked_scenarios.insert(*scenario) {
                return Err(format!("审计失败注入重复登记 {parent} / {scenario}"));
            }

            let registration = format!("isolated_audit_scenario!({parent},{parent}_child,");
            let targets: Vec<_> = test_sources
                .iter()
                .flat_map(|(path, source)| {
                    source.match_indices(&registration).filter_map(move |(offset, call)| {
                        let (target, _) = source[offset + call.len()..].split_once(')')?;
                        Some((*path, target))
                    })
                })
                .collect();
            let [(registration_path, target)] = targets.as_slice() else {
                return Err(format!("{parent} 必须由隔离宏唯一登记"));
            };
            if target.rsplit("::").next() != Some(scenario)
                || !target
                    .rsplit("::")
                    .all(|part| !part.is_empty() && part.bytes().all(identifier_byte))
            {
                return Err(format!("{parent} 的隔离入口必须执行标记的场景 {scenario}"));
            }
            if !source.split_whitespace().collect::<Vec<_>>().windows(2).any(|words| {
                words[0] == "fn"
                    && words[1]
                        .strip_prefix(scenario)
                        .is_some_and(|tail| tail.starts_with('(') || tail.is_empty())
            }) {
                return Err(format!(
                    "{} 的标记必须与场景 {scenario} 的定义同文件",
                    owner.display()
                ));
            }

            // 完整标识符在两棵目录中只应出现于定义及隔离登记。字符串分派、直接调用及别名导入也计入。
            for (path, source) in tests.iter().chain(scenarios) {
                let allowed = usize::from(path == owner) + usize::from(path == *registration_path);
                let uses = source
                    .lines()
                    .filter(|line| !line.trim_start().starts_with(MARKER))
                    .flat_map(|line| {
                        line.split(|ch: char| !ch.is_ascii() || !identifier_byte(ch as u8))
                    })
                    .filter(|name| name == scenario)
                    .count();
                if uses != allowed {
                    return Err(format!(
                        "{} 中 {scenario} 的引用不符合定义与隔离登记（禁止普通分派）",
                        path.display()
                    ));
                }
            }
        }
    }
    if marked_scenarios.is_empty() {
        return Err("寄售审计回滚场景必须登记审计失败注入".into());
    }
    Ok(())
}

#[test]
fn postgres_consignment_audit_failure_injections_are_isolated() {
    let storage = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut tests = Vec::new();
    let mut scenarios = Vec::new();
    read_sources(&storage.join("tests/postgres/consignment"), &mut tests);
    read_sources(&storage.join("src/database/consignment/commands/pg_tests"), &mut scenarios);
    check_isolation(&tests, &scenarios, RUNNER).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn postgres_consignment_audit_guard_rejects_wrong_child_dispatch() {
    let scenario = "// 审计失败注入：postgres_example fixture_audit\nasync fn fixture_audit() {}";
    let scenarios = [(PathBuf::from("scenarios.rs"), scenario.to_owned())];
    let dispatch =
        "isolated_audit_scenario!(postgres_example, postgres_example_child, fixture_audit);";
    let tests = [(PathBuf::from("tests.rs"), dispatch.to_owned())];
    assert!(check_isolation(&tests, &scenarios, RUNNER).is_ok());

    let wrong_runner = RUNNER.replace(
        "run_audit_failure_scenario($scenario)",
        "run_audit_failure_scenario(another_scenario)",
    );
    assert!(
        check_isolation(&tests, &scenarios, &wrong_runner)
            .is_err_and(|error| error.contains("只执行登记的场景"))
    );
    let wrong_entry =
        [(PathBuf::from("tests.rs"), dispatch.replace("fixture_audit", "another_scenario"))];
    assert!(
        check_isolation(&wrong_entry, &scenarios, RUNNER)
            .is_err_and(|error| error.contains("必须执行标记的场景"))
    );
}

#[test]
fn postgres_consignment_audit_guard_rejects_additional_direct_dispatch() {
    let scenario = "// 审计失败注入：postgres_example fixture_audit\nasync fn fixture_audit() {}";
    let scenarios = [(PathBuf::from("scenarios.rs"), scenario.to_owned())];
    let dispatch =
        "isolated_audit_scenario!(postgres_example, postgres_example_child, fixture_audit);";
    let tests = [(PathBuf::from("tests.rs"), dispatch.to_owned())];
    assert!(check_isolation(&tests, &scenarios, RUNNER).is_ok());
    let direct_entries = [
        "#[tokio::test] async fn postgres_direct() { fixture_audit().await; }",
        "scenario!(postgres_direct, \"fixture_audit\");",
    ];
    for entry in direct_entries {
        let tests = [(PathBuf::from("tests.rs"), format!("{dispatch}\n{entry}"))];
        assert!(
            check_isolation(&tests, &scenarios, RUNNER)
                .is_err_and(|error| error.contains("普通分派"))
        );
    }
}
