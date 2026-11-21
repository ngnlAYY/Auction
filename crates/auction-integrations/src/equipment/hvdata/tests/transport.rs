//! 通过回环响应替身核对出站约束、响应分类、时间和解压边界。

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use async_compression::tokio::write::GzipEncoder;
use auction_application::consignment::{
    ListingEquipmentLookup, ListingEquipmentLookupError, ListingLookupSnapshot,
    LookupPermanentReason,
};
use auction_domain::EquipmentKey;
use auction_domain::consignment::ListingEquipmentIdentity;
use auction_test_support::hvdata::{
    Chunk, EQUIPMENT, EquipmentDouble, FIXTURE_EID, FIXTURE_KEY, MISSING_PARAMETER, NOT_FOUND,
    RequestCheck, ResponseScript,
};
use auction_time::{Duration as SignedDuration, Timestamp};
use reqwest::StatusCode;
use reqwest::dns::{Name, Resolve, Resolving};
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderValue, LOCATION, RETRY_AFTER};
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;
use url::Url;

use super::super::dns::{EquipmentDns, checked_addresses, public_address};
use super::super::{HvDataConfigurationError, HvDataEquipmentReader, equipment_client_builder};

type LookupError = ListingEquipmentLookupError;
type Reason = LookupPermanentReason;

const DOUBLE_HOST: &str = "equipment-source.invalid";
const RESPONSE_LIMIT: usize = 65_536;

struct DoubleDns {
    calls: Arc<AtomicUsize>,
    addresses: Vec<SocketAddr>,
    delay: Duration,
}

impl Resolve for DoubleDns {
    fn resolve(&self, name: Name) -> Resolving {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let delay = self.delay;
        let addresses = self.addresses.clone();
        Box::pin(async move {
            if name.as_str() != DOUBLE_HOST {
                return Err(io::Error::other("测试只解析回环替身").into());
            }
            tokio::time::sleep(delay).await;
            checked_addresses(addresses, |ip| ip == Ipv4Addr::LOCALHOST).map_err(Into::into)
        })
    }
}

fn simulated_reader(
    addresses: Vec<SocketAddr>,
    delay: Duration,
    maximum_response_bytes: usize,
    tls: bool,
) -> (HvDataEquipmentReader, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let scheme = if tls { "https" } else { "http" };
    let endpoint = Url::parse(&format!("{scheme}://{DOUBLE_HOST}/equip")).expect("替身固定地址");
    let client = equipment_client_builder()
        .https_only(tls)
        .dns_resolver(Arc::new(DoubleDns { calls: Arc::clone(&calls), addresses, delay }))
        .build()
        .expect("测试客户端");
    (HvDataEquipmentReader { client, endpoint, maximum_response_bytes }, calls)
}

fn deadline_after(milliseconds: i64) -> Timestamp {
    Timestamp::now()
        .checked_add_signed(SignedDuration::milliseconds(milliseconds))
        .expect("测试截止")
}

async fn read(
    reader: &HvDataEquipmentReader,
    world_id: &str,
    deadline: Timestamp,
) -> Result<ListingLookupSnapshot, LookupError> {
    let identity = ListingEquipmentIdentity::new(world_id, FIXTURE_EID).expect("测试编号");
    let key = EquipmentKey::new(FIXTURE_KEY).expect("测试凭证");
    reader.lookup(&identity, &key, deadline).await
}

async fn start(script: ResponseScript) -> EquipmentDouble {
    EquipmentDouble::start(FIXTURE_EID, false, script).await.expect("只监听回环替身")
}

fn client(source: &EquipmentDouble) -> HvDataEquipmentReader {
    simulated_reader(vec![source.address()], Duration::ZERO, RESPONSE_LIMIT, false).0
}

fn verify_request(source: &EquipmentDouble) {
    assert_eq!(
        source.requests(),
        [RequestCheck {
            is_equipment_get: true,
            has_expected_query: true,
            has_credentials_or_referer: false,
        }]
    );
}

fn verify_error(result: Result<ListingLookupSnapshot, LookupError>, expected: LookupError) {
    let error = result.expect_err("预期来源分类");
    assert_eq!(error, expected);
    let message = format!("{error} {error:?}");
    assert!(!message.contains(FIXTURE_KEY));
    assert!(!message.contains(DOUBLE_HOST));
    assert!(!message.contains("/equip?"));
}

#[test]
fn hvdata_configuration_bounds_and_fixed_target() {
    for invalid in [0, 4095, 262_145] {
        assert!(matches!(
            HvDataEquipmentReader::new(invalid),
            Err(HvDataConfigurationError::ResponseLimit)
        ));
    }
    for valid in [4096, RESPONSE_LIMIT, 262_144] {
        let reader = HvDataEquipmentReader::new(valid).expect("固定来源客户端");
        assert_eq!(reader.endpoint.as_str(), "https://hvdata.gisadan.dev/equip");
    }
}

#[test]
fn hvdata_dns_ipv4_and_ipv6_allow_only_public_candidates() {
    for address in [
        "0.0.0.0",
        "0.7.4.3",
        "10.1.2.3",
        "100.64.0.1",
        "127.1.2.3",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.0.1",
        "192.0.0.8",
        "192.0.2.1",
        "192.88.99.1",
        "192.168.0.1",
        "198.18.0.1",
        "198.19.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:10.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::a00:1",
        "100::1",
        "fc00::1",
        "fd00::1",
        "fe80::1",
        "fec0::1",
        "ff02::1",
        "2001:2::1",
        "2001:db8::1",
        "2001:10::1",
        "2002:a00:1::1",
        "3fff:fff::1",
        "5f00::1",
    ] {
        assert!(!public_address(address.parse::<IpAddr>().expect("测试 IP")), "{address}");
    }
    for address in ["8.8.8.8", "1.1.1.1", "100.128.0.1", "2001:4860::1", "2606:4700::1"] {
        assert!(public_address(address.parse::<IpAddr>().expect("测试 IP")), "{address}");
    }
    let good: SocketAddr = "8.8.8.8:0".parse().expect("测试地址");
    let bad: SocketAddr = "127.0.0.1:0".parse().expect("测试地址");
    assert!(checked_addresses([], public_address).is_err());
    assert!(checked_addresses([good, bad], public_address).is_err());
    let result =
        checked_addresses([good], public_address).expect("公网解析结果").collect::<Vec<_>>();
    assert_eq!(result, [good]);
}

#[tokio::test]
async fn hvdata_dns_production_resolver_refuses_other_names_before_resolution() {
    let name: Name = "not-the-fixed-host.invalid".parse().expect("测试域名");
    assert!(EquipmentDns.resolve(name).await.is_err());
}

#[tokio::test]
async fn hvdata_loopback_dns_binds_addresses_and_sends_exact_parameters_without_secrets_in_headers()
{
    for (world_id, isekai) in [("main", false), ("isekai", true)] {
        let source = EquipmentDouble::start(
            FIXTURE_EID,
            isekai,
            ResponseScript::json(StatusCode::OK, EQUIPMENT),
        )
        .await
        .expect("只监听回环替身");
        // URI 使用不可解析的测试域名和缺省端口，只能靠单次解析所得的地址与端口连接。
        let (reader, calls) =
            simulated_reader(vec![source.address()], Duration::ZERO, RESPONSE_LIMIT, false);
        let snapshot =
            read(&reader, world_id, deadline_after(4000)).await.expect("绑定后的回环读取");
        assert_eq!(snapshot.identity().world_id(), world_id);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        verify_request(&source);
    }
}

#[tokio::test]
async fn hvdata_dns_failure_and_timeout_remain_safe() {
    let (reader, calls) = simulated_reader(Vec::new(), Duration::ZERO, RESPONSE_LIMIT, false);
    verify_error(read(&reader, "main", deadline_after(3000)).await, LookupError::SourceUnavailable);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    verify_error(read(&reader, "main", Timestamp::UNIX_EPOCH).await, LookupError::SourceTimeout);
    assert_eq!(calls.load(Ordering::Relaxed), 1);

    let (reader, calls) =
        simulated_reader(Vec::new(), Duration::from_secs(2), RESPONSE_LIMIT, false);
    let started = Instant::now();
    verify_error(read(&reader, "main", deadline_after(300)).await, LookupError::SourceTimeout);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn hvdata_cleared_key_fails_before_any_dns() {
    let (reader, calls) = simulated_reader(Vec::new(), Duration::ZERO, RESPONSE_LIMIT, false);
    let mut key = EquipmentKey::new(FIXTURE_KEY).expect("测试凭证");
    key.clear();
    let identity = ListingEquipmentIdentity::new("main", FIXTURE_EID).expect("测试编号");
    verify_error(
        reader.lookup(&identity, &key, deadline_after(3000)).await,
        LookupError::CredentialUnavailable,
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn hvdata_loopback_http_errors_follow_status_and_validated_body() {
    for (status, body, expected) in [
        (StatusCode::NOT_FOUND, NOT_FOUND, Reason::EquipmentNotFound.into()),
        (StatusCode::NOT_FOUND, br#"{"detail":"Unexpected"}"#, Reason::SourceSchemaInvalid.into()),
        (StatusCode::UNPROCESSABLE_ENTITY, MISSING_PARAMETER, Reason::SourceRequestRejected.into()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            br#"{"detail":[{}]}"#,
            Reason::SourceSchemaInvalid.into(),
        ),
        (StatusCode::BAD_GATEWAY, b"untrusted", LookupError::SourceUnavailable),
        (StatusCode::INTERNAL_SERVER_ERROR, b"untrusted", LookupError::SourceUnavailable),
        (StatusCode::UNAUTHORIZED, EQUIPMENT, Reason::SourceHttpInvalid.into()),
        (StatusCode::CREATED, EQUIPMENT, Reason::SourceHttpInvalid.into()),
    ] {
        let source = start(ResponseScript::json(status, body)).await;
        verify_error(read(&client(&source), "main", deadline_after(3000)).await, expected);
        verify_request(&source);
    }
}

#[tokio::test]
async fn hvdata_loopback_does_not_follow_redirects() {
    let mut script = ResponseScript::json(StatusCode::FOUND, EQUIPMENT);
    script.headers.insert(LOCATION, HeaderValue::from_static("/unrequested"));
    let source = start(script).await;
    verify_error(
        read(&client(&source), "main", deadline_after(3000)).await,
        Reason::SourceHttpInvalid.into(),
    );
    verify_request(&source);
}

#[tokio::test]
async fn hvdata_loopback_retry_after_seconds_date_invalid_and_cap() {
    let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(120));
    for (header, range) in [
        (Some("15"), Some(15..=15)),
        (Some(future.as_str()), Some(115..=120)),
        (Some("100000"), Some(21600..=21600)),
        (Some("invalid"), None),
        (None, None),
    ] {
        let mut script = ResponseScript::json(StatusCode::TOO_MANY_REQUESTS, EQUIPMENT);
        if let Some(header) = header {
            script.headers.insert(RETRY_AFTER, HeaderValue::from_str(header).expect("测试重试头"));
        }
        let source = start(script).await;
        let result = read(&client(&source), "main", deadline_after(3000)).await;
        let error = result.expect_err("限流分类");
        let LookupError::SourceRateLimited(hint) = error else {
            panic!("限流返回其他分类 {error:?}");
        };
        assert_eq!(hint.is_some(), range.is_some());
        if let (Some(hint), Some(range)) = (hint, range) {
            assert!(range.contains(&hint.duration().as_secs()));
        }
        verify_error(Err(error), error);
    }
}

#[tokio::test]
async fn hvdata_loopback_checks_media_type_and_encoding() {
    for (content_type, encoding, accepted) in [
        (Some("application/json"), Some("identity"), true),
        (Some("Application/JSON; charset=\"UTF-8\""), None, true),
        (Some("application/json; charset=utf-8"), None, true),
        (None, None, false),
        (Some("text/json"), None, false),
        (Some("application/json; charset=utf-16"), None, false),
        (Some("application/json; unknown=utf-8"), None, false),
        (Some("application/json; charset=utf-8; charset=utf-8"), None, false),
        (Some("application/json"), Some("br"), false),
    ] {
        let mut script = ResponseScript::json(StatusCode::OK, EQUIPMENT);
        script.headers.remove(CONTENT_TYPE);
        if let Some(content_type) = content_type {
            script.headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        if let Some(encoding) = encoding {
            script.headers.insert(CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
        let source = start(script).await;
        let result = read(&client(&source), "main", deadline_after(3000)).await;
        if accepted {
            assert!(result.is_ok());
        } else {
            verify_error(result, Reason::SourceHttpInvalid.into());
        }
    }
}

#[tokio::test]
async fn hvdata_loopback_decoded_body_byte_limit_is_inclusive_for_identity_and_gzip() {
    for compressed in [false, true] {
        for extra in [0, 1] {
            let mut body = EQUIPMENT.to_vec();
            body.resize(RESPONSE_LIMIT + extra, b' ');
            let body = if compressed {
                let mut encoder = GzipEncoder::new(Vec::new());
                encoder.write_all(&body).await.expect("压缩测试字节");
                encoder.shutdown().await.expect("结束测试压缩");
                let compressed_body = encoder.into_inner();
                assert!(compressed_body.len() < 4096);
                compressed_body
            } else {
                body
            };
            let mut script = ResponseScript::json(StatusCode::OK, body);
            if compressed {
                script.headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
            }
            let source = start(script).await;
            let result = read(&client(&source), "main", deadline_after(3000)).await;
            if extra == 0 {
                assert!(result.is_ok(), "{result:?}");
            } else {
                verify_error(result, Reason::SourceTooLarge.into());
            }
        }
    }
    let mut invalid = ResponseScript::json(StatusCode::OK, EQUIPMENT);
    invalid.headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    let source = start(invalid).await;
    verify_error(
        read(&client(&source), "main", deadline_after(3000)).await,
        Reason::SourceSchemaInvalid.into(),
    );
}

#[tokio::test]
async fn hvdata_loopback_tls_and_interrupted_body_are_transport_failures() {
    let mut broken = ResponseScript::json(StatusCode::OK, EQUIPMENT);
    broken.chunks = vec![
        Chunk::Data(Duration::ZERO, EQUIPMENT[..32].into()),
        Chunk::Disconnect(Duration::from_millis(50)),
    ];
    let source = start(broken).await;
    verify_error(
        read(&client(&source), "main", deadline_after(3000)).await,
        LookupError::SourceUnavailable,
    );
    verify_request(&source);

    let source = start(ResponseScript::json(StatusCode::OK, EQUIPMENT)).await;
    let (reader, calls) =
        simulated_reader(vec![source.address()], Duration::ZERO, RESPONSE_LIMIT, true);
    verify_error(read(&reader, "main", deadline_after(3000)).await, LookupError::SourceUnavailable);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(source.requests().is_empty());
}

#[tokio::test]
async fn hvdata_loopback_deadline_mid_body_stops_before_delayed_chunk() {
    let mut script = ResponseScript::json(StatusCode::OK, EQUIPMENT);
    script.chunks = vec![
        Chunk::Data(Duration::ZERO, EQUIPMENT[..32].into()),
        Chunk::Data(Duration::from_secs(2), EQUIPMENT[32..].into()),
    ];
    let source = start(script).await;
    let started = Instant::now();
    verify_error(
        read(&client(&source), "main", deadline_after(500)).await,
        LookupError::SourceTimeout,
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    verify_request(&source);
}
