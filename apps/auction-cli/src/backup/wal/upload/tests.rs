use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use super::{
    BASE_PREFIX, BASE_UPLOAD_FORMAT, BaseReport, WAL_PREFIX, destination_identity, object_key,
    prune, write_wal_upload_receipt,
};
use crate::backup::wal::model::{
    BASE_MANIFEST_FORMAT, BaseManifest, BaseStaged, WAL_UPLOAD_FORMAT, WalUploadReceipt,
    base_cipher_path, base_manifest_path, base_receipt_path, retention_plan, wal_upload_path,
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "auction-upload-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("创建测试根目录");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("设置测试根目录权限");
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn set(root: &Path, backup_id: &str, cipher: &[u8]) -> BaseStaged {
    let cipher_path = base_cipher_path(root, backup_id);
    let manifest_path = base_manifest_path(root, backup_id);
    fs::write(&cipher_path, cipher).expect("写入密文");
    let record = BaseManifest {
        format: BASE_MANIFEST_FORMAT.to_owned(),
        backup_id: backup_id.to_owned(),
        started_unix_seconds: 1,
        finished_unix_seconds: 2,
        plaintext_bytes: cipher.len() as u64,
        cipher_bytes: cipher.len() as u64,
        cipher_sha256: "0".repeat(64),
    };
    fs::write(&manifest_path, serde_json::to_vec_pretty(&record).expect("序列化 manifest"))
        .expect("写入 manifest");
    BaseStaged {
        backup_id: backup_id.to_owned(),
        cipher: cipher_path,
        manifest: manifest_path,
        record,
        uploaded: true,
        receipt_destination: None,
    }
}

#[test]
fn wal_upload_receipt_is_readable_by_archive_account() {
    let fixture = Fixture::new();
    let receipt = WalUploadReceipt {
        format: WAL_UPLOAD_FORMAT.to_owned(),
        segment_name: "000000010000000000000001".to_owned(),
        source_bytes: 1,
        source_mtime_ns: 2,
        cipher_bytes: 3,
        cipher_sha256: "a".repeat(64),
        remote_key: "wal/000000010000000000000001.zst.age".to_owned(),
        uploaded_unix_seconds: 4,
    };
    write_wal_upload_receipt(&fixture.root, &receipt).expect("写入上传证据");
    let mode = fs::metadata(wal_upload_path(&fixture.root, &receipt.segment_name))
        .expect("读取上传证据")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "postgres archive 账号必须能读取上传证据");
}

#[test]
fn object_keys_prefix_wal_and_base_separately() {
    assert_eq!(object_key("", WAL_PREFIX), "wal");
    assert_eq!(object_key("auction", WAL_PREFIX), "auction/wal");
    assert_eq!(object_key("auction", BASE_PREFIX), "auction/base");
    assert_eq!(
        format!("{}/{}", object_key("auction", BASE_PREFIX), "base-20261104T031500Z"),
        "auction/base/base-20261104T031500Z"
    );
    assert_eq!(
        format!("{}/{}", object_key("auction", WAL_PREFIX), "000000010000000000000001.zst.age"),
        "auction/wal/000000010000000000000001.zst.age"
    );
}

#[test]
fn completion_record_format_is_stable() {
    assert_eq!(BASE_UPLOAD_FORMAT, "auction-base-upload.v1");
}

#[test]
fn base_destination_binds_endpoint_bucket_and_prefix() {
    let original = destination_identity("https://r2.example/", "bucket", "auction/base");
    assert_eq!(original, "https://r2.example\u{241f}bucket\u{241f}auction/base");
    assert_ne!(original, destination_identity("https://other.example", "bucket", "auction/base"));
    assert_ne!(original, destination_identity("https://r2.example", "other", "auction/base"));
    assert_ne!(original, destination_identity("https://r2.example", "bucket", "other/base"));
}

#[test]
fn prune_removes_old_sets_and_keeps_newest_three() {
    let fixture = Fixture::new();
    let root = fixture.root.as_path();
    let ids = [
        "base-20261014T031500Z",
        "base-20261021T031500Z",
        "base-20261028T031500Z",
        "base-20261104T031500Z",
    ];
    let sets: Vec<_> = ids.iter().map(|id| set(root, id, b"cipher")).collect();
    // 最旧的组不只有密文和 manifest，还要删掉本机上传证据。
    fs::write(base_receipt_path(root, ids[0]), b"receipt").expect("写入上传证据");
    let plan = retention_plan(sets, 3, &[]);
    let mut report = BaseReport::default();
    prune(root, plan, &mut report);
    assert_eq!(report.removed, vec!["base-20261014T031500Z".to_owned()]);
    assert_eq!(report.retained.len(), 3);
    assert_eq!(report.failed.len(), 0);
    assert!(!base_cipher_path(root, ids[0]).exists());
    assert!(!base_manifest_path(root, ids[0]).exists());
    assert!(!base_receipt_path(root, ids[0]).exists());
    for id in &ids[1..] {
        assert!(base_cipher_path(root, id).exists());
        assert!(base_manifest_path(root, id).exists());
    }
}

#[test]
fn failed_upload_is_not_pruned() {
    let fixture = Fixture::new();
    let root = fixture.root.as_path();
    let ids = [
        "base-20261014T031500Z",
        "base-20261021T031500Z",
        "base-20261028T031500Z",
        "base-20261104T031500Z",
    ];
    let sets: Vec<_> = ids.iter().map(|id| set(root, id, b"cipher")).collect();
    let plan = retention_plan(sets, 3, &[ids[0].to_owned()]);
    let mut report = BaseReport::default();
    prune(root, plan, &mut report);
    assert!(report.removed.is_empty());
    assert!(base_cipher_path(root, ids[0]).exists());
}
