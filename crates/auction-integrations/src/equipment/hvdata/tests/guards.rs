//! 固定装备来源的代理、TLS 构造策略与诊断边界。

use std::env;
use std::io::{self, Write};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use auction_application::consignment::{
    ListingEquipmentLookup, ListingEquipmentLookupError, LookupPermanentReason,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_test_support::hvdata::{
    Chunk, EQUIPMENT, FIXTURE_EID, FIXTURE_KEY, FIXTURE_OWNER, MISSING_PARAMETER, NOT_FOUND,
    ResponseScript,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;

use super::transport::{
    RESPONSE_LIMIT, assert_no_connection, client, deadline_after, public_answer, read,
    simulated_reader, start, unused_loopback, verify_error, verify_request,
};

type LookupError = ListingEquipmentLookupError;
type Reason = LookupPermanentReason;

const PROXY_CHILD: &str = "AUCTION_HVDATA_PROXY_TEST_CHILD";
const LOG_CHILD: &str = "AUCTION_HVDATA_LOG_TEST_CHILD";
const LOG_CONTROL: &str = "hvdata-test-log-capture-ready";
const BODY_MARKER: &str = "fixture-response-do-not-log";

fn test_child(test_name: &str, marker: &str) -> Command {
    let (_, test_name) = test_name.split_once("::").expect("去掉 crate 名前缀");
    let mut command = Command::new(env::current_exe().expect("当前测试程序"));
    command.arg("--exact").arg(test_name).env(marker, "1");
    command
}

fn assert_child_passed(output: &Output) {
    assert!(
        output.status.success() && String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "子进程未通过指定测试"
    );
}

#[tokio::test]
async fn hvdata_environment_proxy_child_still_connects_directly() {
    if env::var_os(PROXY_CHILD).is_some() {
        let source = start(ResponseScript::json(StatusCode::OK, EQUIPMENT)).await;
        assert!(read(&client(&source), "main", deadline_after(1000)).await.is_ok());
        verify_request(&source);
        return;
    }

    let proxy = unused_loopback();
    let proxy_url = format!("http://{}", proxy.local_addr().expect("假代理回环地址"));
    let mut child = test_child(
        concat!(module_path!(), "::hvdata_environment_proxy_child_still_connects_directly"),
        PROXY_CHILD,
    );
    for name in ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    {
        child.env(name, &proxy_url);
    }
    let output =
        child.env_remove("NO_PROXY").env_remove("no_proxy").output().expect("代理测试子进程");
    assert_no_connection(&proxy);
    assert_child_passed(&output);
}

#[test]
fn hvdata_tls_builders_keep_certificate_and_hostname_verification() {
    // 离线环境没有测试证书生成依赖。源码守卫固定两个实际构造点使用 reqwest 的默认校验。
    let equipment = include_str!("../mod.rs")
        .split("impl ListingEquipmentLookup")
        .next()
        .expect("装备构造源码");
    let outbound = include_str!("../../../outbound/mod.rs")
        .split("#[cfg(test)]")
        .next()
        .expect("受限构造源码");
    for builder in [equipment, outbound] {
        let compact: String =
            builder.chars().filter(|character| !character.is_whitespace()).collect();
        assert!(!compact.contains("danger_accept_invalid_certs(true)"));
        assert!(!compact.contains("danger_accept_invalid_hostnames(true)"));
    }
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("日志缓冲锁").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for LogCapture {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

impl log::Log for LogCapture {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        // 单独收集 reqwest 使用的 log，避免受查找 future 的 tracing Dispatch 隔离影响。
        writeln!(self.clone(), "{}", record.args()).expect("捕获 log 日志");
    }

    fn flush(&self) {}
}

impl LogCapture {
    fn take(&self) -> String {
        let bytes = std::mem::take(&mut *self.0.lock().expect("日志缓冲锁"));
        String::from_utf8(bytes).expect("日志为 UTF-8")
    }

    fn verify_redacted(&self, body: &str, body_marker: &str) {
        let output = self.take();
        for secret in [FIXTURE_KEY, "/equip?", body.trim(), body_marker] {
            assert!(!output.contains(secret), "传输日志包含请求或响应原文");
        }
    }
}

fn capture_logs() -> &'static LogCapture {
    static CAPTURE: OnceLock<LogCapture> = OnceLock::new();
    let capture = CAPTURE.get_or_init(LogCapture::default);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("子进程 tracing 采集");
    log::set_logger(capture).expect("子进程 log 采集");
    log::set_max_level(log::LevelFilter::Trace);
    tracing::trace!("{LOG_CONTROL}");
    log::trace!("{LOG_CONTROL}");
    assert_eq!(capture.take().matches(LOG_CONTROL).count(), 2);
    capture
}

#[tokio::test]
async fn hvdata_transport_logs_exclude_request_secrets_and_response_bodies() {
    if env::var_os(LOG_CHILD).is_none() {
        let output = test_child(
            concat!(
                module_path!(),
                "::hvdata_transport_logs_exclude_request_secrets_and_response_bodies"
            ),
            LOG_CHILD,
        )
        .output()
        .expect("日志测试子进程");
        assert_child_passed(&output);
        return;
    }
    let capture = capture_logs();
    let equipment = std::str::from_utf8(EQUIPMENT).expect("夹具 UTF-8");
    let source = start(ResponseScript::json(StatusCode::OK, EQUIPMENT)).await;
    assert!(read(&client(&source), "main", deadline_after(3000)).await.is_ok());
    verify_request(&source);
    capture.verify_redacted(equipment, FIXTURE_OWNER);

    for (status, body, marker, error) in [
        (StatusCode::NOT_FOUND, NOT_FOUND, "Not Found", Reason::EquipmentNotFound.into()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            MISSING_PARAMETER,
            "Field required",
            Reason::SourceRequestRejected.into(),
        ),
        (
            StatusCode::CREATED,
            BODY_MARKER.as_bytes(),
            BODY_MARKER,
            Reason::SourceHttpInvalid.into(),
        ),
        (StatusCode::OK, BODY_MARKER.as_bytes(), BODY_MARKER, Reason::SourceSchemaInvalid.into()),
        (
            StatusCode::BAD_GATEWAY,
            BODY_MARKER.as_bytes(),
            BODY_MARKER,
            LookupError::SourceUnavailable,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            BODY_MARKER.as_bytes(),
            BODY_MARKER,
            LookupError::SourceRateLimited(None),
        ),
    ] {
        let source = start(ResponseScript::json(status, body)).await;
        verify_error(read(&client(&source), "main", deadline_after(3000)).await, error);
        capture.verify_redacted(std::str::from_utf8(body).expect("夹具 UTF-8"), marker);
    }

    let huge = BODY_MARKER.repeat(RESPONSE_LIMIT.div_ceil(BODY_MARKER.len()) + 1);
    let source = start(ResponseScript::json(StatusCode::OK, huge.clone())).await;
    verify_error(
        read(&client(&source), "main", deadline_after(3000)).await,
        Reason::SourceTooLarge.into(),
    );
    capture.verify_redacted(&huge, BODY_MARKER);

    for (field, value, reason) in [
        ("level", json!(10), Reason::SourceShapeUnconfirmed),
        ("eid", json!(999), Reason::SourceIdentityMismatch),
        ("owner", Value::Null, Reason::OwnerMissing),
    ] {
        let mut body: Value = serde_json::from_str(equipment).expect("装备夹具");
        body[field] = value;
        body["fixture_marker"] = json!(BODY_MARKER);
        let body = body.to_string();
        let source = start(ResponseScript::json(StatusCode::OK, body.clone())).await;
        verify_error(read(&client(&source), "main", deadline_after(3000)).await, reason.into());
        capture.verify_redacted(&body, BODY_MARKER);
    }

    let mut script = ResponseScript::json(StatusCode::OK, BODY_MARKER);
    script.initial_delay = Duration::from_secs(1);
    let source = start(script).await;
    let reader = client(&source);
    verify_error(read(&reader, "main", deadline_after(400)).await, LookupError::SourceTimeout);
    verify_request(&source);
    capture.verify_redacted(BODY_MARKER, BODY_MARKER);

    let identity = ListingEquipmentIdentity::new("main", FIXTURE_EID).expect("测试编号");
    let mut key = EquipmentKey::new(FIXTURE_KEY).expect("测试凭证");
    key.clear();
    verify_error(
        reader.lookup(&identity, &key, deadline_after(3000)).await,
        LookupError::CredentialUnavailable,
    );
    capture.verify_redacted(BODY_MARKER, BODY_MARKER);

    let (reader, _) = simulated_reader(Vec::new(), Duration::ZERO, RESPONSE_LIMIT, false);
    verify_error(read(&reader, "main", deadline_after(3000)).await, LookupError::SourceUnavailable);
    capture.verify_redacted(BODY_MARKER, BODY_MARKER);

    let mut script = ResponseScript::json(StatusCode::OK, BODY_MARKER);
    script.chunks.push(Chunk::Disconnect(Duration::from_millis(50)));
    let source = start(script).await;
    verify_error(
        read(&client(&source), "main", deadline_after(3000)).await,
        LookupError::SourceUnavailable,
    );
    capture.verify_redacted(BODY_MARKER, BODY_MARKER);

    let (reader, _) = simulated_reader(
        vec![public_answer(source.address())],
        Duration::ZERO,
        RESPONSE_LIMIT,
        true,
    );
    verify_error(read(&reader, "main", deadline_after(3000)).await, LookupError::SourceUnavailable);
    capture.verify_redacted(BODY_MARKER, BODY_MARKER);
}
