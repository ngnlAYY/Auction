//! 来源验证回执核对的核心测试：回执必须绑定外层摘要与解压目录清单，新格式包缺回执即拒。

use auction_time::{Duration, Timestamp};

use super::package_receipt::{PackageVerification, Provenance, check, parse};
use super::web_stack_tests::Fixture;
use super::{Artifact, WebStackIdentity, inspect_at};

type Mutation = Box<dyn Fn(&mut PackageVerification)>;

const OUTER: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const NOW: &str = "2026-11-24T12:00:00Z";

fn now() -> Timestamp {
    Timestamp::parse_from_rfc3339(NOW).expect("固定时刻")
}

fn artifact(fixture: &Fixture) -> (Artifact, super::web_stack_tests::TempRoot) {
    let root = fixture.build();
    (inspect_at(root.path(), "0.1.9").expect("自洽发布包"), root)
}

fn receipt(artifact: &Artifact, version: &str) -> PackageVerification {
    PackageVerification {
        archive_sha256: OUTER.to_owned(),
        package: artifact.package.clone(),
        source_commit: artifact.source_commit.clone(),
        tag: format!("v{version}"),
        provenance: Provenance::Verified,
        web_stack_id: artifact
            .web_stack
            .as_ref()
            .map_or_else(|| "none".to_owned(), WebStackIdentity::id),
        verified_at: now() - Duration::hours(1),
    }
}

#[test]
fn matching_receipt_passes_and_new_format_requires_one() {
    let fixture = Fixture::solid();
    let (artifact, _root) = artifact(&fixture);
    let good = receipt(&artifact, fixture.version);
    check(&artifact, Some(&good), OUTER, now()).expect("绑定一致的回执必须通过");
    assert!(check(&artifact, None, OUTER, now()).is_err(), "新格式包缺回执必须拒绝");
}

#[test]
fn old_format_package_may_omit_the_receipt_but_a_given_one_is_still_bound() {
    // 旧版本 Rust 包：无页面凭据。
    let legacy = Fixture::legacy();
    let (artifact, _root) = artifact(&legacy);
    check(&artifact, None, OUTER, now()).expect("旧格式包可无回执");
    let mut stale = receipt(&artifact, legacy.version);
    stale.archive_sha256 = "2".repeat(64);
    assert!(check(&artifact, Some(&stale), OUTER, now()).is_err());
}

#[test]
fn receipt_bindings_are_enforced() {
    let fixture = Fixture::solid();
    let (artifact, _root) = artifact(&fixture);
    let good = receipt(&artifact, fixture.version);
    let cases: [(&str, Mutation); 8] = [
        ("外层摘要不符", Box::new(|r| r.archive_sha256 = "2".repeat(64))),
        ("包名不符", Box::new(|r| r.package.push_str("-x"))),
        ("提交不符", Box::new(|r| r.source_commit = "c".repeat(40))),
        ("tag 不符", Box::new(|r| r.tag = "v9.9.9".to_owned())),
        ("来源证明被跳过", Box::new(|r| r.provenance = Provenance::Skipped)),
        ("页面栈标识不符", Box::new(|r| r.web_stack_id = "none".to_owned())),
        ("回执过期", Box::new(|r| r.verified_at = now() - Duration::hours(25))),
        ("回执时刻在未来", Box::new(|r| r.verified_at = now() + Duration::hours(1))),
    ];
    for (name, mutate) in cases {
        let mut bad = good.clone();
        mutate(&mut bad);
        assert!(check(&artifact, Some(&bad), OUTER, now()).is_err(), "{name}必须拒绝");
    }
}

#[test]
fn receipt_text_is_strictly_parsed() {
    let text = format!(
        "schema=auction-package-verification/1\narchive_sha256={OUTER}\npackage=auction-rust-0.2.0-aaaaaaaaaaaa\nsource_commit={}\ntag=v0.2.0\nprovenance=verified\nweb_stack_id=public-solid_admin-solid\nverified_at={NOW}\n",
        "a".repeat(40)
    );
    let parsed = parse(text.as_bytes()).expect("脚本写出的回执必须能解析");
    assert_eq!(parsed.provenance, Provenance::Verified);
    assert_eq!(parsed.verified_at, now());
    for bad in [
        text.replace("provenance=verified", "provenance=maybe"),
        text.replace("tag=v0.2.0\nprovenance=verified\n", "provenance=verified\ntag=v0.2.0\n"),
        text.replace(
            "web_stack_id=public-solid_admin-solid",
            "web_stack_id=public-solid_admin-dioxus",
        ),
        text.replace(NOW, "not-a-time"),
        format!("{text}extra=1\n"),
        text.trim_end().to_owned(),
    ] {
        assert!(parse(bad.as_bytes()).is_err());
    }
}
