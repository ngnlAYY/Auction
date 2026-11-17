use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use auction_config::{AuctionScheduleConfig, FirstRunConfig, PostgresConnectionConfig};
use clap::Parser;
use uuid::Uuid;

use super::artifact::{Artifact, ArtifactBinary, REQUIRED_BINARIES};
use super::run_inspected;
use crate::cli::{Cli, Command, DeployArgs};
use crate::config_dir::ConfigDir;
use crate::support::digest::sha256_bytes;
use crate::support::files::effective_uid;

/// systemctl 替身：每次调用的参数追加到记录文件；只应答 `Systemctl::show` 的状态查询（三个 unit
/// 都已停止、运行账号为当前 UID、配置绑定到本现场的角色文件），其余调用（停止前读取超时、stop、
/// start）记录后失败退出。
const SYSTEMCTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> '@LOG@'
[ "$1" = show ] && [ "$2" = --property ] || exit 1
case "$5" in
  auction-worker.service) role=worker ;;
  auction-admin-api.service) role=admin ;;
  auction-public-api.service) role=public ;;
  *) exit 1 ;;
esac
printf 'LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nInvocationID=\nUser=@UID@\nDynamicUser=no\nEnvironment=AUCTION_CONFIG_FILE=@CONFIG@/%s.toml\n' "$role"
"#;

/// deploy 入口的本地现场：首次设置向导生成的三角色配置、owner 凭据、安装目录（标准 0755）、停机
/// 备份目录与 systemctl 替身；部署状态目录由部署锁按生产口径创建。数据库指向回环 9 号端口，只有
/// 入口越过安装目录门禁后的时段门禁才会尝试连接；不启动任何真实服务。
struct Site {
    root: PathBuf,
}

impl Site {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("auction-deploy-entry-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("建立临时目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).expect("设置权限");
        let site = Self { root };
        first_run().write_to(&site.path("config")).expect("生成三角色配置");
        write_file(
            &site.path("owner.toml"),
            "database_url = \"postgres://auction_owner:not-a-real-password@127.0.0.1:9/auction\"\n",
            0o600,
        );
        create_dir(&site.path("backups"), 0o700);
        create_dir(&site.path("bin"), 0o755);
        for name in REQUIRED_BINARIES {
            write_file(&site.path(&format!("bin/{name}")), &format!("a-{name}"), 0o755);
        }
        let script = SYSTEMCTL
            .replace("@LOG@", &site.path("systemctl.log").display().to_string())
            .replace("@UID@", &effective_uid().to_string())
            .replace("@CONFIG@", &site.path("config").display().to_string());
        write_file(&site.path("systemctl"), &script, 0o755);
        site
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// 已通过静态校验的新版本制品（程序内容为 `b-<程序名>`）；入口在停机前拒绝时不会读取它们。
    fn artifact(&self) -> Artifact {
        let bin = self.path("artifact/bin");
        fs::create_dir_all(&bin).expect("建立制品目录");
        let binaries = REQUIRED_BINARIES
            .into_iter()
            .map(|name| {
                let content = format!("b-{name}");
                write_file(&bin.join(name), &content, 0o755);
                ArtifactBinary {
                    name,
                    path: bin.join(name),
                    sha256: sha256_bytes(content.as_bytes()),
                    bytes: content.len() as u64,
                }
            })
            .collect();
        Artifact {
            root: self.path("artifact"),
            package: format!("auction-rust-0.1.0-{}", "b".repeat(12)),
            source_commit: "b".repeat(40),
            source_fingerprint: "b".repeat(64),
            binaries,
            public_payload_sha256: "b".repeat(64),
            admin_payload_sha256: "b".repeat(64),
        }
    }

    /// 与运维命令相同的参数解析；只把路径换成本现场。
    fn deploy_args(&self) -> DeployArgs {
        let text = |relative: &str| self.path(relative).display().to_string();
        let cli = Cli::try_parse_from([
            "auction-cli".to_owned(),
            "deploy".to_owned(),
            text("artifact"),
            "--owner-secrets".to_owned(),
            text("owner.toml"),
            "--backup-dir".to_owned(),
            text("backups"),
            "--install-dir".to_owned(),
            text("bin"),
            "--systemctl".to_owned(),
            text("systemctl"),
            "--state-dir".to_owned(),
            text("state"),
            "--actor".to_owned(),
            "operator".to_owned(),
        ])
        .expect("参数可以解析");
        let Command::Deploy(args) = cli.command else { panic!("应解析为 deploy") };
        args
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 首次设置向导的输入；生成的配置能通过 deploy 入口的严格预检。
fn first_run() -> FirstRunConfig {
    FirstRunConfig {
        database: PostgresConnectionConfig {
            host: "127.0.0.1".to_owned(),
            port: 9,
            username: "auction_runtime".to_owned(),
            password: "not-a-real-database-password".to_owned(),
            name: "auction".to_owned(),
        },
        admin_actor: "operator".to_owned(),
        admin_secret: "a".repeat(32),
        public_origin: "https://auction.example.com".to_owned(),
        admin_origin: "https://admin.example.com".to_owned(),
        public_listener_port: 8090,
        admin_listener_port: 8091,
        metrics_enabled: false,
        schedule: AuctionScheduleConfig::default(),
        start_price: 1000,
        log_filter: "info".to_owned(),
        worker_concurrency: 4,
        worker_poll_interval_seconds: 15,
        realtime_poll_interval_millis: 250,
        control_group: "auction".to_owned(),
        control_uid: effective_uid(),
    }
}

fn create_dir(path: &Path, mode: u32) {
    fs::create_dir(path).expect("建立目录");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("设置权限");
}

fn write_file(path: &Path, content: &str, mode: u32) {
    fs::write(path, content).expect("写入文件");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("设置权限");
}

/// 只有安装目录中的旧 auction-cli 缺执行位（0644；三个角色程序、各级目录与运行账号都满足）：deploy
/// 入口按生产顺序走完 owner 凭据、部署锁、严格配置与 unit 绑定、安装目录推导后，在停机前拒绝。
/// systemctl 替身只收到状态查询，没有停止前的超时查询、stop 或 start；没有写回执、回滚记录或停机
/// 备份，也不修改权限。旧门禁放行这种布局，停机后用旧 CLI 校验停机备份时才失败。
#[tokio::test]
async fn deploy_rejects_an_unexecutable_installed_cli_before_stopping_any_unit() {
    let site = Site::new();
    let cli = site.path("bin/auction-cli");
    fs::set_permissions(&cli, fs::Permissions::from_mode(0o644)).expect("去掉执行位");
    let config_dir = ConfigDir::new(&site.path("config")).expect("配置目录");

    let error = run_inspected(&config_dir, &site.artifact(), site.deploy_args())
        .await
        .expect_err("旧 CLI 不能执行时必须在停机前拒绝");
    let text = format!("{error:#}");
    assert!(text.contains("auction-cli 不能由部署执行账号"), "{text}");
    assert!(text.contains("不会自动修改权限"), "{text}");

    let calls = fs::read_to_string(site.path("systemctl.log")).expect("systemctl 调用记录");
    let units =
        ["auction-worker.service", "auction-admin-api.service", "auction-public-api.service"];
    for unit in units {
        assert!(calls.lines().any(|line| line.ends_with(unit)), "门禁前已查询 {unit}：{calls}");
    }
    assert!(
        calls.lines().all(|line| line.starts_with("show --property LoadState,")),
        "停机前只允许状态查询：{calls}"
    );
    assert!(site.path("state").is_dir(), "部署锁已按生产口径固定状态目录");
    assert!(!site.path("state/receipts").exists(), "停机前拒绝不写回执");
    assert!(!site.path("state/rollback").exists(), "停机前拒绝不保留旧程序");
    let backups = fs::read_dir(site.path("backups")).expect("备份目录").count();
    assert_eq!(backups, 0, "停机前拒绝不写停机备份");
    let mode = fs::metadata(&cli).expect("元数据").permissions().mode() & 0o777;
    assert_eq!(mode, 0o644, "门禁不修补权限");
}
