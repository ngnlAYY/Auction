//! 密钥重加密的本地门禁、部分回执与审计范围回归用例；不连接数据库。

use std::{
    cell::Cell,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use auction_config::DataKeyDomain;
use auction_time::Timestamp;
use clap::Parser;
use uuid::Uuid;

use super::equipment::require_zero_old_references;
use super::rewrite::{DomainOutcome, FailureStage, apply_if_requested};
use super::{ProcessedDomain, audit_details, audited, plan, receipt_document};
use crate::cli::{Cli, Command, KeysCommand, KeysReencryptArgs};

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("auction-keys-plan-{}", Uuid::new_v4()));
        fs::create_dir(&path).expect("建立临时目录");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("设置权限");
        Self(path)
    }

    fn receipt(&self) -> String {
        self.0.join("keys.json").display().to_string()
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn parse(arguments: &[&str]) -> Result<KeysReencryptArgs, clap::Error> {
    let mut full = vec!["auction-cli", "keys", "reencrypt"];
    full.extend_from_slice(arguments);
    let cli = Cli::try_parse_from(full)?;
    let Command::Keys(KeysCommand::Reencrypt(args)) = cli.command else {
        panic!("应解析为 keys reencrypt");
    };
    Ok(args)
}

fn plan_error(args: &KeysReencryptArgs) -> String {
    let Err(error) = plan(args) else { panic!("本地门禁必须拒绝") };
    format!("{error:#}")
}

fn processed(
    domain: DataKeyDomain,
    scanned: i64,
    rewritten: i64,
    failure_stage: Option<FailureStage>,
) -> ProcessedDomain {
    ProcessedDomain {
        domain,
        outcome: DomainOutcome { scanned, rewritten, ..DomainOutcome::new(domain) },
        failure_stage,
    }
}

/// `--apply` 缺少回执或操作者是参数错误（退出码 2），不会走到任何数据库操作。
#[test]
fn apply_requires_receipt_and_actor_at_parse_time() {
    for arguments in [
        &["--apply", "--actor", "ops"][..],
        &["--apply", "--receipt", "/var/lib/auction/keys/x.json"][..],
        &["--apply"][..],
    ] {
        let error = parse(arguments).expect_err("--apply 缺少回执或操作者必须被拒绝");
        assert_eq!(error.kind(), clap::error::ErrorKind::MissingRequiredArgument, "{arguments:?}");
    }
    parse(&["--domain", "credential"]).expect("演练不要求回执与操作者");
}

/// 相对路径、已存在的文件与缺失的父目录都在连接数据库之前被拒绝：信封不会在没有回执的
/// 情况下被改写。
#[test]
fn receipt_path_is_checked_before_any_database_access() {
    let args = parse(&["--apply", "--receipt", "keys.json", "--actor", "ops"]).expect("可解析");
    assert!(plan_error(&args).contains("绝对路径"));

    let directory = TempDirectory::new();
    let receipt = directory.receipt();
    let args = parse(&["--apply", "--receipt", &receipt, "--actor", "ops"]).expect("可解析");
    let accepted = plan(&args).expect("私有父目录中的新文件可用");
    assert_eq!(accepted.domains.len(), 3);
    assert_eq!(accepted.apply.as_ref().map(|identity| identity.actor), Some("ops"));

    fs::write(&receipt, b"{}").expect("写入已有文件");
    assert!(plan_error(&args).contains("已存在"));

    let missing = Path::new(&receipt).with_file_name("missing").join("keys.json");
    let missing = missing.display().to_string();
    let args = parse(&["--receipt", &missing]).expect("演练也可带回执");
    plan_error(&args);
}

/// 写入审计的操作者与其他写入命令同一规则：首尾空白或换行一律拒绝（有意收紧）。
#[test]
fn apply_actor_must_be_a_trimmed_single_line() {
    let directory = TempDirectory::new();
    let receipt = directory.receipt();
    for actor in [" ops", "ops ", "ops\nroot", "o\u{7}ps"] {
        let args = parse(&["--apply", "--receipt", &receipt, "--actor", actor])
            .expect("clap 接受任意文本");
        assert!(plan_error(&args).contains("--actor"), "{actor:?}");
    }
}

#[test]
fn mail_diagnostics_domain_is_rejected_before_connecting() {
    let args = parse(&["--domain", "mail-diagnostics"]).expect("可解析");
    assert!(plan_error(&args).contains("mail_diagnostics"));
}

/// 中途失败的回执标记 partial，逐域记录已提交的改写数与是否完整，失败只记域与固定阶段。
#[test]
fn partial_receipt_records_committed_rewrites_and_failure_stage() {
    let items = [
        processed(DataKeyDomain::Equipment, 3, 3, None),
        processed(DataKeyDomain::UserLogin, 5, 2, Some(FailureStage::Conflict)),
    ];
    let document = receipt_document(
        Uuid::new_v4(),
        true,
        Timestamp::now(),
        &items,
        Some((DataKeyDomain::UserLogin, FailureStage::Conflict)),
    );
    assert_eq!(document["outcome"], "partial");
    assert_eq!(document["applied"], true);
    assert_eq!(document["failure"]["domain"], "user_login");
    assert_eq!(document["failure"]["stage"], "conflict");
    assert_eq!(document["domains"]["equipment"]["rewritten"], 3);
    assert_eq!(document["domains"]["equipment"]["complete"], true);
    assert_eq!(document["domains"]["user_login"]["rewritten"], 2);
    assert_eq!(document["domains"]["user_login"]["complete"], false);
    assert!(document["domains"].get("credential").is_none(), "未进入的域不出现在回执里");

    let completed = receipt_document(Uuid::new_v4(), true, Timestamp::now(), &items[..1], None);
    assert_eq!(completed["outcome"], "completed");
    assert!(completed["failure"].is_null());
}

/// 完整处理的域都审计（含幂等重跑的 0 次改写）；中途失败的域在已有改写提交时审计，没有改写
/// 的扫描、重新封装或 CAS 冲突失败不审计。
#[test]
fn audit_covers_completed_domains_and_committed_partial_rewrites() {
    for stage in [FailureStage::Scan, FailureStage::Reseal, FailureStage::Conflict] {
        let items = [
            processed(DataKeyDomain::Equipment, 0, 0, None),
            processed(DataKeyDomain::UserLogin, 4, 0, Some(stage)),
        ];
        let domains = audited(&items).map(|item| item.domain).collect::<Vec<_>>();
        assert_eq!(domains, vec![DataKeyDomain::Equipment], "{stage:?}");
    }

    let items = [
        processed(DataKeyDomain::Equipment, 2, 2, None),
        processed(DataKeyDomain::UserLogin, 4, 1, Some(FailureStage::Conflict)),
    ];
    let domains = audited(&items).map(|item| item.domain).collect::<Vec<_>>();
    assert_eq!(domains, vec![DataKeyDomain::Equipment, DataKeyDomain::UserLogin]);
}

/// 写回阶段失败时那条自动提交的 UPDATE 可能已在服务端提交、重跑不会再扫描到它：即使已确认
/// 的改写数为 0 也必须审计，审计详情标明失败阶段（计数可能少 1）；完整的域阶段为 null。
#[test]
fn audit_covers_write_failure_with_unknown_result() {
    let items = [
        processed(DataKeyDomain::Equipment, 2, 2, None),
        processed(DataKeyDomain::UserLogin, 1, 0, Some(FailureStage::Write)),
    ];
    let audited_items = audited(&items).collect::<Vec<_>>();
    let domains = audited_items.iter().map(|item| item.domain).collect::<Vec<_>>();
    assert_eq!(domains, vec![DataKeyDomain::Equipment, DataKeyDomain::UserLogin]);

    let receipt_id = Uuid::new_v4();
    let partial = audit_details(audited_items[1], "ops", receipt_id, "a", true);
    assert_eq!(partial["outcome"], "partial");
    assert_eq!(partial["failure_stage"], "write");
    assert_eq!(partial["rewritten"], 0);
    assert_eq!(partial["receipt_id"], receipt_id.to_string());
    assert_eq!(partial["actor"], "ops");
    let completed = audit_details(audited_items[0], "ops", receipt_id, "a", true);
    assert_eq!(completed["outcome"], "completed");
    assert!(completed["failure_stage"].is_null());
}

#[tokio::test]
async fn dry_run_does_not_enter_the_database_mutation_boundary() {
    let writes = Cell::new(0);
    for (apply, expected) in [(false, 0), (true, 1)] {
        apply_if_requested(apply, async || {
            writes.set(writes.get() + 1);
            Ok(())
        })
        .await
        .expect("运行离线轮换的写入边界");
        assert_eq!(writes.get(), expected);
    }
}

#[test]
fn equipment_receipt_and_audit_keep_three_source_counts_without_key_metadata() {
    let mut item = processed(DataKeyDomain::Equipment, 0, 0, None);
    for _ in 0..3 {
        item.outcome.scanned("equipment_secrets");
        item.outcome.rewritten("equipment_secrets");
    }
    for _ in 0..5 {
        item.outcome.scanned("consignment_registration_secrets");
    }
    for _ in 0..2 {
        item.outcome.rewritten("consignment_registration_secrets");
    }
    let receipt_id = Uuid::new_v4();
    let document =
        receipt_document(receipt_id, true, Timestamp::now(), std::slice::from_ref(&item), None);
    let domain = &document["domains"]["equipment"];
    let counts = &domain["sources"];
    assert_eq!(document["format"], "auction-key-reencrypt-receipt-v2");
    assert_eq!(
        counts,
        &serde_json::json!({
            "equipment_secrets": {"scanned": 3, "rewritten": 3},
            "consignment_registration_secrets": {"scanned": 5, "rewritten": 2},
            "consignment_replay_keys": {"scanned": 0, "rewritten": 0},
        })
    );
    let audit = audit_details(&item, "ops", receipt_id, "a", true);
    assert_eq!(&audit["sources"], counts);
    assert!(document.get("from_key_ids").is_none());
    assert!(domain.get("current_key_id").is_none());
    assert!(domain.get("to_key_id").is_none());
    assert!(domain.get("from_key_ids").is_none());
    assert!(audit.get("from_key_ids").is_none());
    assert!(audit.get("current_key_id").is_none());
    assert!(audit.get("to_key_id").is_none());
}

#[test]
fn retirement_check_rejects_existing_old_references_even_when_current_exists() {
    const CURRENT: &str = "fake-current-key";
    const OLD: &str = "fake-old-key";
    assert!(require_zero_old_references(&[], CURRENT).is_ok());
    assert!(require_zero_old_references(&[(CURRENT.to_owned(), 4)], CURRENT).is_ok());
    let error =
        require_zero_old_references(&[(CURRENT.to_owned(), 4), (OLD.to_owned(), 1)], CURRENT)
            .expect_err("在线退休前必须没有旧引用");
    assert_eq!(error.to_string(), "装备域仍有 1 个信封引用旧钥，请先完成重加密");
}

#[test]
fn equipment_verification_is_an_explicit_read_only_status_mode() {
    let cli = Cli::try_parse_from(["auction-cli", "keys", "status", "--require-current-equipment"])
        .expect("解析退休前核对参数");
    assert!(matches!(
        cli.command,
        Command::Keys(KeysCommand::Status { strict: false, require_current_equipment: true })
    ));
    let error = Cli::try_parse_from([
        "auction-cli",
        "keys",
        "status",
        "--strict",
        "--require-current-equipment",
    ])
    .expect_err("互斥的状态模式不能混用");
    assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
}
