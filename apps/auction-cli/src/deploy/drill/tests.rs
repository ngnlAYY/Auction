use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Requirements, inspect};
use crate::deploy::artifact::{Artifact, ArtifactBinary};

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn artifact() -> Artifact {
    Artifact {
        root: PathBuf::from("/srv/artifact"),
        package: "auction-rust-0.1.0-aaaaaaaaaaaa".to_owned(),
        source_commit: "a".repeat(40),
        source_fingerprint: "b".repeat(64),
        binaries: vec![ArtifactBinary {
            name: "auction-cli",
            path: PathBuf::from("/srv/artifact/bin/auction-cli"),
            sha256: "c".repeat(64),
            bytes: 1,
        }],
        public_payload_sha256: "d".repeat(64),
        admin_payload_sha256: "e".repeat(64),
    }
}

fn receipt() -> Value {
    json!({
        "format": "auction-migration-drill.v1",
        "drill_id": "drill-20261105",
        "supersedes_drill_id": null,
        "operator": "operator",
        "reviewer": "reviewer",
        "date_utc": "2026-11-05",
        "started_at": "2026-11-05T03:00:00Z",
        "finished_at": "2026-11-05T04:00:00Z",
        "artifact": {
            "version": "0.1.0",
            "commit": "a".repeat(40),
            "package": "auction-rust-0.1.0-aaaaaaaaaaaa",
            "package_sha256": "0".repeat(64),
            "cli_sha256": "c".repeat(64),
            "previous_version": "schema-94",
        },
        "backup": {
            "created_at": "2026-11-05T03:05:00Z",
            "schema_version": 94,
            "database_name_sha256": "1".repeat(64),
            "files": [
                {"kind": "archive", "name": "a.dump", "sha256": "2".repeat(64)},
                {"kind": "manifest", "name": "a.dump.manifest", "sha256": "3".repeat(64)},
            ],
            "verify_result": "verified",
            "verify_evidence": "backup-verify.json",
        },
        "postgres": {
            "major": 18,
            "server_version": "18.6",
            "isolated": true,
            "instance_label": "rehearsal",
            "host": "127.0.0.1",
            "port": 55470,
            "database": "auction_test",
        },
        "restore": {
            "result": "passed",
            "elapsed_seconds": 30,
            "receipt": "restore.json",
            "preflight_evidence": "restored-preflight.json",
        },
        "migration": {
            "before_version": 94,
            "after_version": 96,
            "expected_version": 96,
            "applied": 2,
            "started_at": "2026-11-05T03:10:00Z",
            "finished_at": "2026-11-05T03:11:00Z",
            "elapsed_seconds": 60,
            "exit_code": 0,
            "result": "passed",
            "evidence": "migrate.json",
            "timing_evidence": "migrate-time.txt",
        },
        "readiness": {
            "budget_seconds_per_role": 30,
            "worker": {"result": "passed", "http_status": 200, "exit_code": 0, "checked_at": "2026-11-05T03:12:00Z", "pid": 10, "evidence": "worker-ready.json"},
            "admin": {"result": "passed", "http_status": 200, "exit_code": 0, "checked_at": "2026-11-05T03:12:00Z", "pid": 11, "evidence": "admin-ready.json"},
            "public": {"result": "passed", "http_status": 200, "exit_code": 0, "checked_at": "2026-11-05T03:12:00Z", "pid": 12, "evidence": "public-ready.json"},
        },
        "consistency_check": {
            "work_item": "W11.4",
            "result": "passed",
            "evidence": "consistency.json",
            "note": "W11.4 巡检通过",
        },
        "downtime": {
            "started_at": "2026-11-05T03:09:00Z",
            "drained_at": "2026-11-05T03:09:30Z",
            "ready_at": "2026-11-05T03:12:00Z",
            "measured_seconds": 180,
            "estimated_seconds": 200,
            "estimate_basis": "drain + backup + install + readiness",
            "minimum_window_minutes": 10,
            "selected_window_minutes": 60,
        },
        "evidence_files": [{"name": "restore.json", "sha256": "4".repeat(64)}],
        "anomalies": [],
        "conclusion": {"result": "passed", "summary": "可进入部署"},
    })
}

fn write(value: &Value) -> TempFile {
    let path = std::env::temp_dir().join(format!("auction-drill-{}.json", uuid::Uuid::new_v4()));
    std::fs::write(&path, serde_json::to_vec_pretty(value).expect("序列化")).expect("写入");
    TempFile(path)
}

/// 与回执夹具同一天的固定校验时刻；新鲜度窗口按注入时钟计算，测试不读真实时钟。
fn fixture_now() -> chrono::DateTime<chrono::Utc> {
    "2026-11-05T12:00:00Z".parse().expect("fixture time")
}

fn requirements<'a>(artifact: &'a Artifact) -> Requirements<'a> {
    Requirements {
        artifact,
        current_version: 94,
        expected_version: 96,
        protection_window_minutes: 60,
        max_age_days: 7,
        now: fixture_now(),
    }
}

fn check_at(
    value: &Value,
    artifact: &Artifact,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<super::DrillEvidence> {
    let file = write(value);
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let mut requirements = requirements(artifact);
    requirements.now = now;
    runtime.block_on(inspect(Path::new(&file.0), &requirements))
}

fn check(value: &Value, artifact: &Artifact) -> anyhow::Result<super::DrillEvidence> {
    let file = write(value);
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    runtime.block_on(inspect(Path::new(&file.0), &requirements(artifact)))
}

#[test]
fn receipt_freshness_follows_the_injected_clock() {
    let artifact = artifact();
    let day = |text: &str| -> chrono::DateTime<chrono::Utc> { text.parse().expect("time") };
    // 回执日 2026-11-05 加 7 天窗口（含当日余量）到 11-13 00:00 为止仍有效。
    assert!(check_at(&receipt(), &artifact, day("2026-11-13T00:00:00Z")).is_ok());
    let stale = check_at(&receipt(), &artifact, day("2026-11-13T00:00:01Z")).expect_err("过期");
    assert!(stale.to_string().contains("超过 7 天"));
    let future = check_at(&receipt(), &artifact, day("2026-11-04T23:59:59Z")).expect_err("未来");
    assert!(future.to_string().contains("日期在未来"));
}

#[test]
fn valid_receipt_passes_and_reports_interval() {
    let artifact = artifact();
    let evidence = check(&receipt(), &artifact).expect("有效回执必须通过");
    assert_eq!(evidence.drill_id, "drill-20261105");
    assert_eq!(evidence.minimum_window_minutes, 10);
    assert_eq!(evidence.before_version, 94);
    assert_eq!(evidence.after_version, 96);
}

#[test]
fn candidate_identity_and_interval_must_match() {
    let artifact = artifact();
    let mut value = receipt();
    value["artifact"]["cli_sha256"] = json!("9".repeat(64));
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["migration"]["after_version"] = json!(95);
    value["migration"]["applied"] = json!(1);
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["artifact"]["commit"] = json!("f".repeat(40));
    assert!(check(&value, &artifact).is_err());
}

#[test]
fn incomplete_or_unreviewed_receipts_are_rejected() {
    let artifact = artifact();

    let mut value = receipt();
    value["operator"] = json!("reviewer");
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["readiness"]["admin"]["result"] = json!("not_run");
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["backup"]["verify_result"] = json!("not_run");
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["anomalies"] = json!([{"note": "unexplained"}]);
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["conclusion"]["result"] = json!("incomplete");
    assert!(check(&value, &artifact).is_err());

    // 已落地，not_available 不再能通过演练回执门禁。
    let mut value = receipt();
    value["consistency_check"]["result"] = json!("not_available");
    value["consistency_check"]["note"] = json!("W11.4 尚未落地");
    assert!(check(&value, &artifact).is_err());
}

#[test]
fn window_budget_must_cover_three_times_downtime() {
    let artifact = artifact();
    let mut value = receipt();
    value["downtime"]["minimum_window_minutes"] = json!(4);
    assert!(check(&value, &artifact).is_err());

    let mut value = receipt();
    value["downtime"]["selected_window_minutes"] = json!(5);
    assert!(check(&value, &artifact).is_err());

    // 保护窗口小于回执下限时也拒绝；这里下限由 200 秒 × 3 得出 10 分钟。
    let file = write(&receipt());
    let runtime =
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let mut requirements = requirements(&artifact);
    requirements.protection_window_minutes = 5;
    let error = runtime.block_on(inspect(Path::new(&file.0), &requirements)).expect_err("窗口不足");
    assert!(error.to_string().contains("protection-window-minutes"));
}
