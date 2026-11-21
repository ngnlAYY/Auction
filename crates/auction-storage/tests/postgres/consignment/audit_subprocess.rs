//! 会触发 `record_audit_failure()` 的场景必须经本模块在子进程运行。
//! 在注入处用 `// 审计失败注入：<父测试函数名>` 登记。父测试只调用运行器，
//! 同文件中的子测试使用 `_child` 后缀并标记为忽略，由运行器精确选中。

use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TEST_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// 在新的测试进程中运行指定的忽略用例。调用方传 `module_path!()` 与子测试短名。
pub(super) fn run_audit_failure_test(test_module: &str, child_test: &str) {
    let (_, module) = test_module
        .split_once("::")
        .unwrap_or_else(|| panic!("子测试路径应包含集成目标的 crate 名"));
    run_test(&format!("{module}::{child_test}"), TEST_TIMEOUT)
        .unwrap_or_else(|error| panic!("{error}"));
}

fn run_test(test_name: &str, timeout: Duration) -> Result<(), String> {
    let executable =
        env::current_exe().map_err(|error| format!("定位当前测试程序失败：{error}"))?;
    let (mut reader, writer) =
        output_socket().map_err(|error| format!("准备子测试输出失败：{error}"))?;
    let stderr = writer.try_clone().map_err(|error| format!("复制子测试输出失败：{error}"))?;
    // 已配置的 AUCTION_TEST_DATABASE_URL 由子进程继承。夹具没有公开自建集群的地址，
    // 未配置时子进程由同一夹具自建集群与模板库，不在父测试里额外启动集群。
    let mut child = Command::new(executable)
        .args(["--exact", test_name, "--ignored", "--nocapture", "--color", "never"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .stderr(Stdio::from(OwnedFd::from(stderr)))
        .spawn()
        .map_err(|error| format!("启动子测试 {test_name} 失败：{error}"))?;

    let mut output = Vec::new();
    let deadline = Instant::now() + timeout;
    let status = match wait_for_test(&mut child, &mut reader, &mut output, deadline) {
        Ok(Some(status)) => status,
        outcome => {
            child.kill().map_err(|error| format!("终止子测试 {test_name} 失败：{error}"))?;
            child.wait().map_err(|error| format!("回收子测试 {test_name} 失败：{error}"))?;
            let reason = match outcome {
                Ok(_) => format!("超过 {timeout:?}"),
                Err(error) => format!("读取子测试状态或输出失败：{error}"),
            };
            return Err(format!("子测试 {test_name} {reason}{}", panic_site(test_name, &output)));
        }
    };
    read_output(&mut reader, &mut output)
        .map_err(|error| format!("读取子测试 {test_name} 输出失败：{error}"))?;
    if !status.success() {
        return Err(format!(
            "子测试 {test_name} 退出失败（{status}）{}",
            panic_site(test_name, &output)
        ));
    }
    // libtest 在没有匹配到忽略用例时也返回 0，必须同时确认恰有一个用例实际通过。
    if !String::from_utf8_lossy(&output)
        .lines()
        .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
    {
        return Err(format!("子测试 {test_name} 未实际运行并通过一条忽略用例"));
    }
    Ok(())
}

fn output_socket() -> io::Result<(UnixStream, UnixStream)> {
    let (reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    Ok((reader, writer))
}

fn wait_for_test(
    child: &mut Child,
    reader: &mut UnixStream,
    output: &mut Vec<u8>,
    deadline: Instant,
) -> io::Result<Option<ExitStatus>> {
    loop {
        read_output(reader, output)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

fn read_output(reader: &mut UnixStream, output: &mut Vec<u8>) -> io::Result<()> {
    // 每次读取设预算，持续输出的子进程也不能阻止检查截止时间。
    match reader.take(256 * 1024).read_to_end(output) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(error) => return Err(error),
    }
    output.drain(..output.len().saturating_sub(OUTPUT_TAIL_BYTES));
    Ok(())
}

fn panic_site(test_name: &str, output: &[u8]) -> String {
    // 不转发断言值、任意标准输出或环境信息，只取 libtest panic 头中的源码位置。
    let thread_prefix = format!("thread '{test_name}' ");
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| line.strip_prefix(&thread_prefix))
        .filter_map(|line| line.split_once("panicked at "))
        .filter_map(|(_, site)| site.strip_suffix(':'))
        .find(|site| {
            site.rsplit_once(".rs:").is_some_and(|(file, position)| {
                !file.is_empty()
                    && file
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte))
                    && position.split_once(':').is_some_and(|(line, column)| {
                        line.parse::<u32>().is_ok() && column.parse::<u32>().is_ok()
                    })
            })
        })
        .map_or_else(String::new, |site| format!("，断言位置 {site}"))
}

#[test]
#[ignore = "由运行器的测试作为故障夹具启动"]
fn postgres_consignment_subprocess_probe_child() {
    thread::sleep(Duration::from_millis(200));
    panic!("FAKE_SECRET_MUST_NOT_REACH_PARENT");
}

#[test]
fn postgres_consignment_subprocess_rejects_failure_empty_selection_and_timeout() {
    let (_, module) = module_path!().split_once("::").unwrap_or_else(|| panic!("集成目标路径"));
    let child = format!("{module}::postgres_consignment_subprocess_probe_child");
    let failure =
        run_test(&child, TEST_TIMEOUT).err().unwrap_or_else(|| panic!("子测试非零退出必须失败"));
    assert!(failure.contains("退出失败") && failure.contains("audit_subprocess.rs:"));
    assert!(!failure.contains("FAKE_SECRET_MUST_NOT_REACH_PARENT"));

    let missing = format!("{module}::postgres_missing_ignored_child");
    assert!(run_test(&missing, TEST_TIMEOUT).is_err_and(|error| error.contains("未实际运行")));
    assert!(run_test(&child, Duration::ZERO).is_err_and(|error| error.contains("超过")));
}

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

#[test]
fn postgres_consignment_audit_failure_injections_are_isolated() {
    let storage = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut tests = Vec::new();
    let mut scenarios = Vec::new();
    read_sources(&storage.join("tests/postgres/consignment"), &mut tests);
    read_sources(&storage.join("src/database/consignment/commands/pg_tests"), &mut scenarios);

    let normalized: Vec<String> = tests
        .iter()
        .map(|(_, source)| source.split_whitespace().collect::<String>().replace(",)", ")"))
        .collect();
    let marker = "// 审计失败注入：";
    let ignored = "#[ignore=\"由父测试在子进程中运行：会写入进程级审计健康计数\"]";
    let mut injections = HashSet::new();
    for (path, source) in tests.iter().chain(&scenarios) {
        for parent in source.lines().filter_map(|line| line.trim_start().strip_prefix(marker)) {
            let parent = parent.trim();
            assert!(
                parent.starts_with("postgres_")
                    && parent.bytes().all(|byte| byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'_'),
                "{} 中的审计失败注入必须登记父测试函数名",
                path.display()
            );
            assert!(injections.insert(parent), "审计失败注入重复登记父测试 {parent}");
            let child = format!("{parent}_child");
            let wrapper = format!(
                "#[test]fn{parent}(){{run_audit_failure_test(module_path!(),\"{child}\");}}"
            );
            let async_test = format!("#[tokio::test]{ignored}asyncfn{child}(){{");
            let sync_test = format!("#[test]{ignored}fn{child}(){{");
            assert!(
                normalized.iter().any(|source| {
                    source.contains(&wrapper)
                        && (source.contains(&async_test) || source.contains(&sync_test))
                }),
                "{} 中登记的审计失败注入 {parent} 必须由父测试启动同文件的忽略子测试",
                path.display()
            );
        }
    }
    assert!(!injections.is_empty(), "寄售审计回滚场景必须登记审计失败注入");
}
