//! 核对装配根共用的寄售启用判定：只有挂载根下的回执登记了本程序摘要与构建号才开启。

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use uuid::Uuid;

use super::activation::{test_activation, test_own_binary_sha256};
use crate::{
    AppConfig, ConfigError, ConsignmentActivation, Role, WEB_STACK_RECEIPT_FILE, WebStackBinary,
    process_consignment_activation,
};

/// 装配根传入的本进程构建号；夹具用固定假值。
const BUILD: &str = "build-fixture";

struct Fixture(PathBuf);

impl Fixture {
    /// 公共文件开启寄售；`mount_root` 为真时把挂载根指向夹具目录。
    fn new(mount_root: bool) -> Self {
        let directory = std::env::temp_dir().join(format!("auction-activation-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let mut common = String::from("[consignment]\nenabled = true\n");
        if mount_root {
            common.push_str(&format!("[web]\nmount_root = '{}'\n", directory.display()));
        }
        fs::write(directory.join("common.toml"), common).unwrap();
        fs::write(directory.join("secrets.toml"), "").unwrap();
        fs::write(
            directory.join("public.toml"),
            "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n",
        )
        .unwrap();
        Self(directory)
    }

    fn config(&self) -> AppConfig {
        AppConfig::load_from_path(Role::Public, self.0.join("public.toml")).unwrap()
    }

    fn write_receipt(&self, receipt: &Value) {
        fs::write(self.0.join(WEB_STACK_RECEIPT_FILE), receipt.to_string()).unwrap();
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            assert!(std::thread::panicking(), "清理启用判定夹具失败：{error}");
        }
    }
}

fn surface(build: &str) -> Value {
    json!({
        "stack": "solid",
        "payload_sha256": "a".repeat(64),
        "entry_build_id": build,
        "entry_script_policy": "solid",
        "legacy": { "build_id": "build-legacy", "payload_sha256": "b".repeat(64), "script_policy": "dioxus" }
    })
}

/// 回执只为 `registered` 指定的程序登记本测试进程的真实摘要，其余程序登记别的摘要。
fn receipt(registered: &str) -> Value {
    let own = test_own_binary_sha256().unwrap();
    let other = "c".repeat(64);
    let sha = |name: &str| if name == registered { own.clone() } else { other.clone() };
    let build = BUILD;
    json!({
        "schema": "auction-web-stack-receipt/1",
        "web_stack_id": "public-solid_admin-solid",
        "build_id": build,
        "surfaces": { "public": surface(build), "admin": surface(build) },
        "binaries": {
            "auction-public-api": { "path": "/x", "sha256": sha("auction-public-api") },
            "auction-admin-api": { "path": "/x", "sha256": sha("auction-admin-api") },
            "auction-worker": { "path": "/x", "sha256": sha("auction-worker") },
            "auction-cli": { "path": "/x", "sha256": sha("auction-cli") },
            "auction-server": if registered == "auction-server" { json!({ "path": "/x", "sha256": own }) } else { Value::Null }
        }
    })
}

fn rejected(result: Result<ConsignmentActivation, ConfigError>) {
    assert!(matches!(result, Err(ConfigError::ConsignmentWebStack(_))), "{result:?}");
}

/// 发布构型：功能完成、无预览、非调试。
fn release(
    config: &AppConfig,
    binary: WebStackBinary,
) -> Result<ConsignmentActivation, ConfigError> {
    test_activation(config, binary, BUILD, true, false, false)
}

#[test]
fn registered_receipt_enables_only_the_matching_program() {
    let fixture = Fixture::new(true);
    fixture.write_receipt(&receipt("auction-cli"));
    let config = fixture.config();
    assert_eq!(config.web.mount_root(), Some(fixture.path()));
    assert_eq!(release(&config, WebStackBinary::Cli).unwrap(), ConsignmentActivation::Enabled);
    // 同一回执里 Public 登记的是别的程序摘要：混装的程序不能借用 CLI 的登记开启寄售。
    rejected(release(&config, WebStackBinary::Public));

    let mut other_build = receipt("auction-cli");
    other_build["build_id"] = json!("build-other");
    fixture.write_receipt(&other_build);
    rejected(release(&config, WebStackBinary::Cli));
}

/// 合并部署：回执的 `auction-server` 为 null（CLI 现在的写法）时，即使子角色摘要都登记了也拒绝开启。
#[test]
fn merged_server_without_a_registered_digest_fails_closed() {
    let fixture = Fixture::new(true);
    let config = fixture.config();
    let mut without_server = receipt("auction-public-api");
    without_server["binaries"]["auction-server"] = Value::Null;
    fixture.write_receipt(&without_server);
    rejected(release(&config, WebStackBinary::Server));
    fixture.write_receipt(&receipt("auction-server"));
    assert_eq!(release(&config, WebStackBinary::Server).unwrap(), ConsignmentActivation::Enabled);
}

#[test]
fn release_builds_without_a_receipt_fail_closed() {
    for mount_root in [false, true] {
        let fixture = Fixture::new(mount_root);
        let config = fixture.config();
        // 非调试或无预览 feature 的任一组合都不享有例外：缺回执即拒绝。
        for (preview, debug) in [(false, false), (true, false), (false, true)] {
            rejected(test_activation(&config, WebStackBinary::Public, BUILD, true, preview, debug));
        }
        // 制品门先于回执：功能未完成的正式制品仍报原错误，不读回执。
        assert!(matches!(
            process_consignment_activation(&config, WebStackBinary::Public, BUILD, false, false),
            Err(ConfigError::ConsignmentPreviewRequired)
        ));
    }
}

#[test]
fn debug_preview_is_the_only_build_exempt_from_the_receipt() {
    let fixture = Fixture::new(false);
    let config = fixture.config();
    for complete in [false, true] {
        assert_eq!(
            test_activation(&config, WebStackBinary::Public, BUILD, complete, true, true).unwrap(),
            ConsignmentActivation::Enabled
        );
    }
    fs::write(fixture.path().join("common.toml"), "[consignment]\nenabled = false\n").unwrap();
    assert_eq!(
        process_consignment_activation(
            &fixture.config(),
            WebStackBinary::Public,
            BUILD,
            true,
            true
        )
        .unwrap(),
        ConsignmentActivation::Disabled
    );
}

#[test]
fn mount_root_must_be_a_canonical_absolute_path() {
    let fixture = Fixture::new(false);
    for root in ["relative/web", "/srv/web/../other", "/"] {
        fs::write(fixture.path().join("common.toml"), format!("[web]\nmount_root = '{root}'\n"))
            .unwrap();
        let error = AppConfig::load_from_path(Role::Public, fixture.path().join("public.toml"))
            .expect_err(root);
        assert!(matches!(error, ConfigError::InvalidWebMountRoot), "{root}: {error:?}");
        assert!(!error.to_string().contains(root));
    }
    for contents in ["[web]\nunknown = 1\n", "web = ['/srv/web']\n"] {
        fs::write(fixture.path().join("common.toml"), contents).unwrap();
        let error = AppConfig::load_from_path(Role::Public, fixture.path().join("public.toml"))
            .expect_err(contents);
        assert!(matches!(error, ConfigError::Parse { .. }), "{contents}: {error:?}");
    }
}
