//! 用替身测试进程验证精确匹配、脱敏失败诊断和超时后的父进程清理。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use uuid::Uuid;

use super::{TEST_TIMEOUT, run_test, test_command, test_path};

const RESOURCE_PATH: &str = "AUCTION_AUDIT_TEST_PROBE_RESOURCE";

#[test]
#[ignore = "由运行器的测试作为故障夹具启动"]
fn postgres_consignment_subprocess_probe_child() {
    thread::sleep(Duration::from_millis(200));
    panic!("FAKE_SECRET_MUST_NOT_REACH_PARENT");
}

#[test]
#[ignore = "由运行器的测试作为近名夹具启动"]
fn postgres_consignment_subprocess_near_match_present_child() {}

#[test]
#[ignore = "由运行器的测试作为近名夹具启动"]
fn postgres_consignment_subprocess_near_match_other_child() {}

#[tokio::test]
async fn postgres_consignment_subprocess_rejects_failure_empty_selection_and_timeout() {
    let child = test_path(module_path!(), "postgres_consignment_subprocess_probe_child");
    let command = test_command(&child).expect("构造失败夹具命令");
    let failure = run_test(command, &child, TEST_TIMEOUT, async { Ok(()) })
        .await
        .expect_err("子测试非零退出必须失败");
    assert!(failure.contains("退出失败") && failure.contains("probes.rs:"));
    assert!(!failure.contains("FAKE_SECRET_MUST_NOT_REACH_PARENT"));

    let missing = test_path(module_path!(), "postgres_missing_ignored_child");
    let command = test_command(&missing).expect("构造零选中夹具命令");
    let result = run_test(command, &missing, TEST_TIMEOUT, async { Ok(()) }).await;
    assert!(result.is_err_and(|error| error.contains("未实际运行")));

    let command = test_command(&child).expect("构造超时夹具命令");
    let result = run_test(command, &child, Duration::ZERO, async { Ok(()) }).await;
    assert!(result.is_err_and(|error| error.contains("超过")));
}

#[tokio::test]
async fn postgres_consignment_subprocess_rejects_near_names() {
    let missing = test_path(module_path!(), "postgres_consignment_subprocess_near_match_present");
    let command = test_command(&missing).expect("构造近名夹具命令");
    let result = run_test(command, &missing, TEST_TIMEOUT, async { Ok(()) }).await;
    assert!(result.is_err_and(|error| error.contains("未实际运行")), "运行器误接收近名");

    let multiple = test_path(module_path!(), "postgres_consignment_subprocess_near_match");
    let mut command = Command::new(env::current_exe().expect("当前测试程序"));
    command.args([&multiple, "--ignored", "--color", "never"]);
    let result = run_test(command, &multiple, TEST_TIMEOUT, async { Ok(()) }).await;
    assert!(result.is_err_and(|error| error.contains("未实际运行")), "运行器误接收多个用例");
}

struct ProbeResource(PathBuf);

impl Drop for ProbeResource {
    fn drop(&mut self) {
        if self.0.exists() {
            fs::remove_file(&self.0).expect("回收探针文件");
        }
    }
}

#[test]
#[ignore = "由运行器的超时清理测试启动"]
fn postgres_consignment_subprocess_resource_probe_child() {
    let resource = ProbeResource(PathBuf::from(env::var_os(RESOURCE_PATH).expect("探针资源路径")));
    fs::write(&resource.0, b"created by child").expect("创建探针资源");
    // 模拟一个比测试子进程多存活片刻的下级进程。它不会再接触资源文件。
    let status = Command::new("sleep")
        .arg("4")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("运行探针下级进程");
    assert!(status.success());
}

#[tokio::test]
async fn postgres_consignment_subprocess_timeout_cleans_child_resources() {
    let resource =
        ProbeResource(PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(Uuid::new_v4().to_string()));
    let child = test_path(module_path!(), "postgres_consignment_subprocess_resource_probe_child");
    let mut command = test_command(&child).expect("构造持有资源的探针命令");
    command.env(RESOURCE_PATH, &resource.0);
    let result = run_test(command, &child, Duration::from_secs(2), async {
        assert_eq!(fs::read(&resource.0).expect("超时前子进程已经建成资源"), b"created by child");
        fs::remove_file(&resource.0).map_err(|error| format!("父进程清理探针资源失败：{error}"))
    })
    .await;
    assert!(result.is_err_and(|error| error.contains("超过") && !error.contains("清理")));
    assert!(!resource.0.exists(), "超时返回前父进程必须清理子进程持有的资源");
}
