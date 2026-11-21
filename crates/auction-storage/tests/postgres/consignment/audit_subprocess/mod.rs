//! 会触发 `record_audit_failure()` 的场景必须经本模块在子进程运行。
//! 在场景定义处标记 `// 审计失败注入：<父测试名> <场景函数名>`，
//! 用 `isolated_audit_scenario!` 一次生成两个入口，子测试使用 `_child` 后缀。

mod fixture;
mod guard;
mod probes;

use std::env;
use std::future::Future;
use std::io::{self, Read};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use auction_storage::DatabaseStore;

use self::fixture::{ParentFixture, open_child_database};

const TEST_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// 在父进程完成夹具初始化后，运行指定的忽略子测试。
pub(super) async fn run_audit_failure_test(test_module: &str, child_test: &str) {
    let child = test_path(test_module, child_test);
    let fixture = ParentFixture::prepare().await.unwrap_or_else(|error| panic!("{error}"));
    fixture.run(&child).await.unwrap_or_else(|error| panic!("{error}"));
}

/// 用父夹具的临时库执行隔离宏登记的场景。
pub(super) async fn run_audit_failure_scenario(
    scenario: impl AsyncFn(&DatabaseStore, &DatabaseStore) -> Result<(), String>,
) {
    let (store, maintenance) =
        open_child_database().await.unwrap_or_else(|error| panic!("{error}"));
    assert!(scenario(&store, &maintenance).await.is_ok(), "寄售审计失败场景返回错误");
}

fn test_path(module: &str, test: &str) -> String {
    let (_, module) = module.split_once("::").unwrap_or_else(|| panic!("集成目标路径"));
    format!("{module}::{test}")
}

fn test_command(test_name: &str) -> Result<Command, String> {
    let executable =
        env::current_exe().map_err(|error| format!("定位当前测试程序失败：{error}"))?;
    let mut command = Command::new(executable);
    command.args(["--exact", test_name, "--ignored", "--nocapture", "--color", "never"]);
    Ok(command)
}

async fn run_test(
    command: Command,
    test_name: &str,
    timeout: Duration,
    cleanup: impl Future<Output = Result<(), String>>,
) -> Result<(), String> {
    let owned_name = test_name.to_owned();
    let execution = tokio::task::spawn_blocking(move || run_child(command, &owned_name, timeout))
        .await
        .map_err(|error| format!("子测试等待任务失败：{error}"))
        .and_then(|result| result);
    let cleaned = cleanup.await;
    match (execution, cleaned) {
        (Err(execution), Err(cleanup)) => Err(format!("{execution}；{cleanup}")),
        (Ok(()), result) | (result, Ok(())) => result,
    }
}

fn run_child(mut command: Command, test_name: &str, timeout: Duration) -> Result<(), String> {
    let (mut reader, writer) =
        output_socket().map_err(|error| format!("准备子测试输出失败：{error}"))?;
    let stderr = writer.try_clone().map_err(|error| format!("复制子测试输出失败：{error}"))?;
    let mut child = command
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
            if let Err(error) = child.kill()
                && !child.try_wait().is_ok_and(|status| status.is_some())
            {
                return Err(format!("终止子测试 {test_name} 失败：{error}"));
            }
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
        .rev()
        .find(|line| line.starts_with("test result: "))
        .is_some_and(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;"))
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

/// 由同一场景函数生成父测试及忽略子测试。场景接收运行账号、维护账号，返回 `Result<(), String>`。
macro_rules! isolated_audit_scenario {
    ($parent:ident, $child:ident, $scenario:path) => {
        #[tokio::test]
        async fn $parent() {
            $crate::consignment::audit_subprocess::run_audit_failure_test(
                module_path!(),
                stringify!($child),
            )
            .await;
        }

        #[tokio::test]
        #[ignore = "由父测试在子进程中运行：会写入进程级审计健康计数"]
        async fn $child() {
            $crate::consignment::audit_subprocess::run_audit_failure_scenario($scenario).await;
        }
    };
}

pub(super) use isolated_audit_scenario;
