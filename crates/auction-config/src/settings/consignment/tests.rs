//! 通过三角色 loader 核对寄售配置的默认值、错误脱敏与制品门。

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

use auction_domain::consignment::ListingPublicationTtls;
use uuid::Uuid;

use crate::{
    AppConfig, CONSIGNMENT_COMPLETE, ConfigError, ConsignmentActivation, ConsignmentConfig, Role,
    consignment_activation,
};

struct ConfigFixture(PathBuf);

impl ConfigFixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("auction-consignment-{}", Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let fixture = Self(directory);
        fs::write(fixture.0.join("secrets.toml"), "").unwrap();
        for role in [Role::Public, Role::Admin, Role::Worker] {
            fs::write(
                fixture.role_file(role),
                "common_file = 'common.toml'\nsecrets_file = 'secrets.toml'\n",
            )
            .unwrap();
        }
        fixture
    }

    fn role_file(&self, role: Role) -> PathBuf {
        self.0.join(format!("{}.toml", role.name()))
    }

    fn write_common(&self, contents: &str) {
        fs::write(self.0.join("common.toml"), contents).unwrap();
    }

    fn load(&self, role: Role) -> Result<ConsignmentConfig, ConfigError> {
        AppConfig::load_from_path(role, self.role_file(role)).map(|config| config.consignment)
    }
}

impl Drop for ConfigFixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            assert!(std::thread::panicking(), "清理寄售配置夹具失败：{error}");
        }
    }
}

fn assert_defaults(config: ConsignmentConfig) {
    assert!(!config.enabled());
    assert_eq!(config.fee_bps().value(), 500);
    assert_eq!(config.publication_ttls(), ListingPublicationTtls::new(30, 7).unwrap());
    assert_eq!(config.reservation_ttl_hours(), 24);
    assert_eq!(config.isekai_preclose_days(), 3);
    assert_eq!(config.max_open_per_user(), 20);
    assert_eq!(config.registrations_per_hour(), 10);
    assert_eq!(config.lookup_timeout_seconds(), 10);
    assert_eq!(config.lookup_max_response_bytes(), 65_536);
    assert_eq!(config.lookup_min_interval_ms(), 1_000);
}

#[test]
fn omitted_and_empty_consignment_use_contract_defaults() {
    let fixture = ConfigFixture::new();
    for contents in ["[readiness]\ncache_enabled = false\n", "[consignment]\n"] {
        fixture.write_common(contents);
        for role in [Role::Public, Role::Admin, Role::Worker] {
            assert_defaults(fixture.load(role).unwrap());
        }
    }
}

#[test]
fn consignment_requires_a_toml_table() {
    let fixture = ConfigFixture::new();
    for contents in [
        "consignment = []",
        "consignment = [true]",
        "consignment = [true, 125, 45, 14, 35, 18, 12, 131072, 2500]",
        "[[consignment]]\nenabled = true",
        "consignment = true",
        "consignment = 1",
        "consignment = 1.5",
        "consignment = 2026-11-20",
        "consignment = 'FAKE-CONSIGNMENT-SCALAR-SECRET'",
    ] {
        fixture.write_common(contents);
        for role in [Role::Public, Role::Admin, Role::Worker] {
            let error = fixture.load(role).expect_err("寄售节的非表形状应拒绝加载");
            assert!(matches!(error, ConfigError::InvalidConsignmentShape { .. }), "{role:?}");
            let message = error.to_string();
            assert!(message.contains("consignment 必须是 TOML 表"));
            for diagnostic in [message, format!("{error:?}")] {
                assert!(!diagnostic.contains(contents));
                assert!(!diagnostic.contains("FAKE-CONSIGNMENT-SCALAR-SECRET"));
            }
        }
    }

    fixture.write_common("consignment = { enabled = true }");
    assert!(fixture.load(Role::Public).unwrap().enabled());
}

#[test]
fn three_roles_load_the_same_consignment_overrides() {
    let fixture = ConfigFixture::new();
    fixture.write_common(
        "[consignment]
enabled = true
fee_bps = 125
display_ttl_days = 45
deposit_ttl_days = 14
reservation_ttl_hours = 36
isekai_preclose_days = 5
max_open_per_user = 35
registrations_per_hour = 18
lookup_timeout_seconds = 12
lookup_max_response_bytes = 131072
lookup_min_interval_ms = 2500
",
    );
    for role in [Role::Public, Role::Admin, Role::Worker] {
        let config = fixture.load(role).unwrap();
        assert!(config.enabled(), "{role:?}");
        assert_eq!(config.fee_bps().value(), 125, "{role:?}");
        assert_eq!(
            config.publication_ttls(),
            ListingPublicationTtls::new(45, 14).unwrap(),
            "{role:?}"
        );
        assert_eq!(config.reservation_ttl_hours(), 36, "{role:?}");
        assert_eq!(config.isekai_preclose_days(), 5, "{role:?}");
        assert_eq!(config.max_open_per_user(), 35, "{role:?}");
        assert_eq!(config.registrations_per_hour(), 18, "{role:?}");
        assert_eq!(config.lookup_timeout_seconds(), 12, "{role:?}");
        assert_eq!(config.lookup_max_response_bytes(), 131_072, "{role:?}");
        assert_eq!(config.lookup_min_interval_ms(), 2_500, "{role:?}");
    }
}

#[test]
fn consignment_numeric_bounds_are_inclusive_and_errors_are_redacted() {
    let fixture = ConfigFixture::new();
    for enabled in [false, true] {
        let section = format!("[consignment]\nenabled = {enabled}\n");
        for (key, min, max) in [
            ("fee_bps", 0, 9_999),
            ("display_ttl_days", 1, 90),
            ("deposit_ttl_days", 1, 30),
            ("reservation_ttl_hours", 1, 168),
            ("isekai_preclose_days", 1, 90),
            ("max_open_per_user", 1, 100),
            ("registrations_per_hour", 1, 100),
            ("lookup_timeout_seconds", 1, 30),
            ("lookup_max_response_bytes", 4_096, 262_144),
            ("lookup_min_interval_ms", 250, 60_000),
        ] {
            for value in [min, max] {
                fixture.write_common(&format!("{section}{key} = {value}\n"));
                let config = fixture.load(Role::Worker).unwrap();
                assert_eq!(config.enabled(), enabled);
                match key {
                    "fee_bps" => assert_eq!(i64::from(config.fee_bps().value()), value),
                    "display_ttl_days" => assert_eq!(
                        config.publication_ttls(),
                        ListingPublicationTtls::new(value, 7).unwrap()
                    ),
                    "deposit_ttl_days" => assert_eq!(
                        config.publication_ttls(),
                        ListingPublicationTtls::new(30, value).unwrap()
                    ),
                    "reservation_ttl_hours" => {
                        assert_eq!(config.reservation_ttl_hours(), value);
                    }
                    "isekai_preclose_days" => {
                        assert_eq!(config.isekai_preclose_days(), u8::try_from(value).unwrap());
                    }
                    "max_open_per_user" => assert_eq!(config.max_open_per_user(), value),
                    "registrations_per_hour" => assert_eq!(config.registrations_per_hour(), value),
                    "lookup_timeout_seconds" => {
                        assert_eq!(config.lookup_timeout_seconds(), u64::try_from(value).unwrap());
                    }
                    "lookup_max_response_bytes" => {
                        assert_eq!(
                            config.lookup_max_response_bytes(),
                            usize::try_from(value).unwrap()
                        );
                    }
                    "lookup_min_interval_ms" => {
                        assert_eq!(config.lookup_min_interval_ms(), u64::try_from(value).unwrap());
                    }
                    _ => panic!("未覆盖的寄售配置键 {key}"),
                }
            }
            for value in [min - 1, max + 1] {
                let assignment = format!("{key} = {value}");
                fixture.write_common(&format!("{section}{assignment}\n"));
                let error = fixture
                    .load(Role::Worker)
                    .expect_err(&format!("enabled = {enabled} 时应拒绝 {assignment}"));
                let message = error.to_string();
                assert!(message.contains(&format!("[consignment].{key}")), "{message}");
                assert!(message.contains(&format!("{min}..={max}")), "{message}");
                assert!(!message.contains(&assignment));
                assert!(!format!("{error:?}").contains(&assignment));
            }
        }
    }
}

#[test]
fn unknown_consignment_keys_are_rejected() {
    const FAKE_SECRET: &str = "FAKE-UNKNOWN-CONSIGNMENT-VALUE-7F83";
    let fixture = ConfigFixture::new();
    let secret_assignment = format!("fake_unknown_key = '{FAKE_SECRET}'");
    for assignment in [
        secret_assignment.as_str(),
        "display_only = true",
        "lookup_max_attempts = 6",
        "lookup_backoff_seconds = [30, 120, 600, 1800, 7200]",
        "lookup_deadline_hours = 24",
        "lookup_endpoint = 'https://example.invalid/equip'",
        "lookup_schema_version = 1",
    ] {
        fixture.write_common(&format!("[consignment]\n{assignment}\n"));
        let error = fixture.load(Role::Public).expect_err("未知寄售键应拒绝加载");
        assert!(matches!(error, ConfigError::Parse { .. }));
        assert_eq!(
            (error.to_string().contains(FAKE_SECRET), format!("{error:?}").contains(FAKE_SECRET)),
            (false, false),
            "Display / Debug 应隐去未知键的值"
        );
    }
}

#[test]
fn wrong_consignment_types_are_rejected_without_echoing_toml() {
    let fixture = ConfigFixture::new();
    for (key, value) in [
        ("enabled", "\"yes\""),
        ("fee_bps", "5.0"),
        ("enabled", "\"FAKE-TEST-SECRET-NOT-A-CREDENTIAL\""),
    ] {
        let assignment = format!("{key} = {value}");
        fixture.write_common(&format!("[consignment]\n{assignment}\n"));
        let error = fixture.load(Role::Admin).expect_err("类型错误应拒绝加载");
        assert!(matches!(error, ConfigError::Parse { .. }));
        for diagnostic in [error.to_string(), format!("{error:?}")] {
            assert!(!diagnostic.contains(&assignment));
            assert!(!diagnostic.contains(value.trim_matches('"')));
        }
    }
}

#[test]
fn consignment_section_is_rejected_in_role_files() {
    let fixture = ConfigFixture::new();
    fixture.write_common("");
    for role in [Role::Public, Role::Admin, Role::Worker] {
        let path = fixture.role_file(role);
        let mut contents = fs::read_to_string(&path).unwrap();
        contents.push_str("[consignment]\nenabled = true\n");
        fs::write(&path, contents).unwrap();
        assert!(matches!(fixture.load(role), Err(ConfigError::Parse { .. })), "{role:?}");
    }
}

#[test]
fn consignment_activation_covers_all_sixteen_combinations() {
    use ConsignmentActivation::{Disabled, Enabled};

    for (enabled, complete, preview, debug, expected) in [
        (false, false, false, false, Some(Disabled)),
        (false, false, false, true, Some(Disabled)),
        (false, false, true, false, Some(Disabled)),
        (false, false, true, true, Some(Disabled)),
        (false, true, false, false, Some(Disabled)),
        (false, true, false, true, Some(Disabled)),
        (false, true, true, false, Some(Disabled)),
        (false, true, true, true, Some(Disabled)),
        (true, false, false, false, None),
        (true, false, false, true, None),
        (true, false, true, false, None),
        (true, false, true, true, Some(Enabled)),
        (true, true, false, false, Some(Enabled)),
        (true, true, false, true, Some(Enabled)),
        (true, true, true, false, Some(Enabled)),
        (true, true, true, true, Some(Enabled)),
    ] {
        let result = consignment_activation(enabled, complete, preview, debug);
        let facts = (enabled, complete, preview, debug);
        if let Some(outcome) = expected {
            assert_eq!(result.unwrap(), outcome, "{facts:?}");
        } else {
            let error = result.expect_err(&format!("{facts:?} 应拒绝开启"));
            assert!(matches!(error, ConfigError::ConsignmentPreviewRequired));
            let message = error.to_string();
            assert!(message.contains("[consignment].enabled 改回 false"));
            assert!(message.contains("consignment-dev-preview 的本地调试构建"));
        }
    }
}

#[test]
fn current_consignment_marker_rejects_enablement_in_official_builds() {
    assert!(matches!(
        consignment_activation(true, CONSIGNMENT_COMPLETE, false, false),
        Err(ConfigError::ConsignmentPreviewRequired)
    ));
}
