//! 首次密钥强度、部署安全警告与原子轮换后的重新加载回归。

use self::parent::*;
use super as parent;

fn fixture() -> FirstRunConfig {
    FirstRunConfig {
        database: PostgresConnectionConfig {
            host: "127.0.0.1".to_owned(),
            port: 5432,
            username: "test_admin_security".to_owned(),
            password: "not-a-real-database-password".to_owned(),
            name: "test_admin_security".to_owned(),
        },
        admin_actor: "operator".to_owned(),
        admin_secret: "a".repeat(32),
        public_origin: "https://auction.example.com".to_owned(),
        admin_origin: "https://admin.example.com".to_owned(),
        public_listener_port: 8090,
        admin_listener_port: 8091,
        metrics_enabled: true,
        schedule: AuctionScheduleConfig::default(),
        start_price: 1000,
        log_filter: "info".to_owned(),
        worker_concurrency: 4,
        worker_poll_interval_seconds: 15,
        realtime_poll_interval_millis: 250,
        control_group: "auction".to_owned(),
        control_uid: 1000,
    }
}

#[test]
fn first_run_requires_32_byte_admin_secret() {
    let mut input = fixture();
    assert!(input.validate().is_ok());
    input.admin_secret = "a".repeat(31);
    assert!(input.validate().is_err());
    input.admin_secret = format!("{}\t", "a".repeat(32));
    assert!(input.validate().is_err());
}

#[test]
fn security_warnings_and_secret_rotation_reload_without_exposing_secret() {
    // 仅物化临时配置；不连接数据库、真实账号或应用服务。
    let directory = std::env::temp_dir().join(format!("auction-admin-security-{}", Uuid::new_v4()));
    fs::DirBuilder::new().mode(0o700).create(&directory).unwrap();
    let config_dir = directory.join("config");
    fixture().write_to(&config_dir).unwrap();
    let load = || AppConfig::load_from_path(Role::Admin, config_dir.join("admin.toml")).unwrap();
    let mut config = load();
    assert!(config.admin_security_warnings().is_empty());
    config.http_security.trusted_proxy_cidrs.clear();
    config.http_security.trusted_unix_proxy = false;
    assert_eq!(config.admin_security_warnings().len(), 1);
    config.http_security.cookie_secure = false;
    assert!(config.admin_security_warnings().is_empty());

    let secrets_path = config_dir.join("secrets.toml");
    let mut secrets: Value = toml::from_str(&fs::read_to_string(&secrets_path).unwrap()).unwrap();
    secrets["admin_secret"] = Value::String("a".repeat(16));
    fs::write(&secrets_path, toml::to_string(&secrets).unwrap()).unwrap();
    let weak = load();
    assert_eq!(weak.admin_security_warnings().len(), 1);
    assert!(weak.admin_security_warnings()[0].contains("32"));
    assert!(!weak.admin_security_warnings()[0].contains(weak.admin_secret().unwrap()));

    // 轮换经原子 rename 发布；现存配置对象保持旧快照，重新加载只接受新材料。
    let replacement = "b".repeat(64);
    secrets["admin_secret"] = Value::String(replacement.clone());
    let pending = config_dir.join("secrets.pending");
    let mut file =
        OpenOptions::new().write(true).create_new(true).mode(0o600).open(&pending).unwrap();
    file.write_all(toml::to_string(&secrets).unwrap().as_bytes()).unwrap();
    file.sync_all().unwrap();
    fs::rename(&pending, &secrets_path).unwrap();
    let rotated = load();
    assert_eq!(rotated.admin_secret(), Some(replacement.as_str()));
    assert_ne!(weak.admin_secret(), rotated.admin_secret());
    assert!(rotated.admin_security_warnings().is_empty());
    fs::remove_dir_all(directory).unwrap();
}
