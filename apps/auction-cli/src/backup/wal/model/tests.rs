use std::path::Path;

use super::paths::{base_id_from_file_name, valid_base_id};
use super::{
    ArchiveManifest, BASE_MANIFEST_FORMAT, BASE_RECEIPT_FORMAT, BaseManifest, BaseReceipt,
    BaseStaged, Duplicate, MANIFEST_FORMAT, SourceIdentity, WAL_UPLOAD_FORMAT, WalUploadReceipt,
    base_cipher_path, base_id, base_manifest_path, base_receipt_path, classify, retention_plan,
    staged, valid_recipient, valid_segment_name, wal_upload_path,
};

#[test]
fn segment_names_match_postgresql_forms() {
    assert!(valid_segment_name("000000010000000000000001"));
    assert!(valid_segment_name("00000002.history"));
    assert!(valid_segment_name("000000010000000000000001.00000028.backup"));
    assert!(!valid_segment_name("00000001000000000000000"));
    assert!(!valid_segment_name("00000001000000000000000Z"));
    assert!(!valid_segment_name("00000001.history.bak"));
    assert!(!valid_segment_name("000000010000000000000001.zst.age"));
    assert!(!valid_segment_name("../000000010000000000000001"));
}

#[test]
fn recipients_require_age1_shape() {
    let valid = "age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
    assert_eq!(valid.len(), 62);
    assert!(valid_recipient(valid));
    assert!(!valid_recipient("AGE1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ"));
    assert!(!valid_recipient("age1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqb"));
    assert!(!valid_recipient("--recipient"));
    assert!(!valid_recipient(""));
}

#[test]
fn manifest_validation_binds_segment_and_length() {
    let record = ArchiveManifest {
        format: MANIFEST_FORMAT.to_owned(),
        segment_name: "000000010000000000000001".to_owned(),
        source_bytes: 16 * 1024 * 1024,
        source_mtime_ns: 1,
        cipher_bytes: 4096,
        archived_unix_seconds: 2,
    };
    assert!(record.validate("000000010000000000000001").is_ok());
    assert!(record.validate("000000010000000000000002").is_err());

    let mut empty = record.clone();
    empty.cipher_bytes = 0;
    assert!(empty.validate("000000010000000000000001").is_err());

    let mut wrong_format = record;
    wrong_format.format = "auction-wal-archive.v0".to_owned();
    assert!(wrong_format.validate("000000010000000000000001").is_err());
}

#[test]
fn durable_upload_proof_classifies_without_cipher_or_manifest() {
    let directory =
        std::env::temp_dir().join(format!("auction-wal-upload-proof-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).expect("建立临时暂存目录");
    let segment = "000000010000000000000001";
    let source = SourceIdentity { bytes: 128, mtime_ns: 7 };
    let receipt = WalUploadReceipt {
        format: WAL_UPLOAD_FORMAT.to_owned(),
        segment_name: segment.to_owned(),
        source_bytes: source.bytes,
        source_mtime_ns: source.mtime_ns,
        cipher_bytes: 64,
        cipher_sha256: "a".repeat(64),
        remote_key: format!("wal/{segment}.zst.age"),
        uploaded_unix_seconds: 1,
    };
    std::fs::write(
        wal_upload_path(&directory, segment),
        serde_json::to_vec(&receipt).expect("序列化上传证据"),
    )
    .expect("写入上传证据");

    match classify(&directory, segment, &source).expect("读取上传证据") {
        Duplicate::Uploaded(record) => assert_eq!(record.cipher_bytes, 64),
        other => panic!("应识别为已上传：{other:?}"),
    }
    let changed = SourceIdentity { bytes: 128, mtime_ns: 8 };
    assert!(classify(&directory, segment, &changed).is_err());
    let _ = std::fs::remove_dir_all(&directory);
}

fn base_record(backup_id: &str) -> BaseManifest {
    BaseManifest {
        format: BASE_MANIFEST_FORMAT.to_owned(),
        backup_id: backup_id.to_owned(),
        started_unix_seconds: 10,
        finished_unix_seconds: 20,
        plaintext_bytes: 1024,
        cipher_bytes: 512,
        cipher_sha256: "0".repeat(64),
    }
}

fn staged_base(backup_id: &str) -> BaseStaged {
    let directory = Path::new("/nonexistent");
    BaseStaged {
        backup_id: backup_id.to_owned(),
        cipher: base_cipher_path(directory, backup_id),
        manifest: base_manifest_path(directory, backup_id),
        record: base_record(backup_id),
        uploaded: false,
        receipt_destination: None,
    }
}

#[test]
fn base_ids_follow_derived_timestamp_form() {
    assert!(valid_base_id("base-20261104T031500Z"));
    assert_eq!(
        base_id(
            chrono::DateTime::parse_from_rfc3339("2026-11-04T03:15:00Z")
                .expect("解析时刻")
                .with_timezone(&chrono::Utc)
        ),
        "base-20261104T031500Z"
    );
    assert!(!valid_base_id("base-20261104T031500"));
    assert!(!valid_base_id("base-2026110T031500Z"));
    assert!(!valid_base_id("base-20261104t031500Z"));
    assert!(!valid_base_id("000000010000000000000001"));
    assert!(!valid_base_id("../base-20261104T031500Z"));

    assert_eq!(
        base_id_from_file_name("base-20261104T031500Z.tar.zst.age"),
        Some("base-20261104T031500Z")
    );
    assert_eq!(
        base_id_from_file_name("base-20261104T031500Z.tar.zst.age.manifest.json"),
        Some("base-20261104T031500Z")
    );
    assert_eq!(base_id_from_file_name("000000010000000000000001.zst.age"), None);
    assert_eq!(
        base_id_from_file_name("base-20261104T031500Z.upload.json"),
        Some("base-20261104T031500Z")
    );
    assert_eq!(base_id_from_file_name("base-backup.lock"), None);
}

#[test]
fn base_manifest_validation_binds_identity_shape_and_length() {
    let record = base_record("base-20261104T031500Z");
    assert!(record.validate("base-20261104T031500Z").is_ok());
    assert!(record.validate("base-20261111T031500Z").is_err());

    let mut empty = record.clone();
    empty.cipher_bytes = 0;
    assert!(empty.validate("base-20261104T031500Z").is_err());

    let mut unordered = record.clone();
    unordered.finished_unix_seconds = 1;
    assert!(unordered.validate("base-20261104T031500Z").is_err());

    let mut bad_digest = record;
    bad_digest.cipher_sha256 = "z".repeat(64);
    assert!(bad_digest.validate("base-20261104T031500Z").is_err());
}

#[test]
fn retention_keeps_newest_sets_and_extends_for_failed_ids() {
    let complete = vec![
        staged_base("base-20261014T031500Z"),
        staged_base("base-20261021T031500Z"),
        staged_base("base-20261028T031500Z"),
        staged_base("base-20261104T031500Z"),
    ];
    let plan = retention_plan(complete.clone(), 3, &[]);
    assert_eq!(plan.retained.len(), 3);
    assert_eq!(plan.removable.len(), 1);
    assert_eq!(plan.removable[0].backup_id, "base-20261014T031500Z");
    assert_eq!(plan.retained[0], "base-20261104T031500Z");

    let plan = retention_plan(complete, 3, &["base-20261014T031500Z".to_owned()]);
    assert!(plan.removable.is_empty());
    assert_eq!(plan.retained.len(), 4);

    // 少于保留份数时不删除任何项。
    let plan = retention_plan(vec![staged_base("base-20261104T031500Z")], 3, &[]);
    assert!(plan.removable.is_empty());
}

#[test]
fn staged_marks_base_sets_uploaded_only_with_valid_receipt() {
    use std::fs;

    let root = std::env::temp_dir().join(format!(
        "auction-wal-staged-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    fs::create_dir(&root).expect("创建测试目录");
    let id = "base-20261104T031500Z";
    fs::write(base_cipher_path(&root, id), b"cipher").expect("写入密文");
    let record = base_record(id);
    let record = BaseManifest { cipher_bytes: 6, plaintext_bytes: 16, ..record };
    fs::write(base_manifest_path(&root, id), serde_json::to_vec(&record).expect("序列化"))
        .expect("写入 manifest");

    let scan = staged(&root).expect("首次扫描");
    assert_eq!(scan.base.len(), 1);
    assert!(!scan.base[0].uploaded);

    let receipt = BaseReceipt {
        format: BASE_RECEIPT_FORMAT.to_owned(),
        backup_id: id.to_owned(),
        destination: "https://example.invalid\u{241f}bucket\u{241f}prefix/base".to_owned(),
        uploaded_unix_seconds: 3,
        completion_sha256: "1".repeat(64),
    };
    let receipt_path = base_receipt_path(&root, id);
    fs::write(&receipt_path, serde_json::to_vec(&receipt).expect("序列化证据")).expect("写入证据");
    let scan = staged(&root).expect("带证据扫描");
    assert!(scan.base[0].uploaded);
    assert_eq!(
        scan.base[0].receipt_destination.as_deref(),
        Some("https://example.invalid\u{241f}bucket\u{241f}prefix/base")
    );

    // 身份不符的证据不能把未上传的组当成已上传。
    let wrong = BaseReceipt { backup_id: "base-20261111T031500Z".to_owned(), ..receipt };
    fs::write(&receipt_path, serde_json::to_vec(&wrong).expect("序列化证据")).expect("覆盖证据");
    let scan = staged(&root).expect("错误证据扫描");
    assert!(!scan.base[0].uploaded);

    // 损坏的证据按未上传处理，不使整次扫描失败。
    fs::write(&receipt_path, b"not-json").expect("写入损坏证据");
    let scan = staged(&root).expect("损坏证据扫描");
    assert!(!scan.base[0].uploaded);

    fs::remove_dir_all(&root).expect("清理测试目录");
}
