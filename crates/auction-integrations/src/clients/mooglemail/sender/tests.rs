//! Sent / Read Mail 扫描检查点的持久化、作用域隔离与回退回归。

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use auction_application::core::{ApplicationError, RepositoryError};
use auction_application::mail::{
    MAIL_SCAN_DAILY_FULL_INTERVAL, MailScanCheckpoint, MailScanCheckpointFuture,
    MailScanCheckpointKey, MailScanCheckpointStore, MailScanMode, MailScanPlan, MailScanReason,
    MailScanStream, MailScanWatermark, plan_mail_scan,
};
use chrono::{TimeZone, Utc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;
use uuid::Uuid;

use self::parent::*;
use crate::clients::mooglemail::MoogleMailClientConfig;
use crate::clients::mooglemail::parser::MoogleMailParser;
use crate::clients::mooglemail::sender as parent;

#[derive(Default)]
struct FakeStore {
    entries: Mutex<Vec<(MailScanCheckpointKey, MailScanCheckpoint)>>,
    saved: Mutex<Vec<(MailScanCheckpointKey, MailScanCheckpoint)>>,
    fail_load: bool,
    fail_save: bool,
}

impl FakeStore {
    fn empty() -> Self {
        Self::default()
    }

    fn with_checkpoint(key: MailScanCheckpointKey, checkpoint: MailScanCheckpoint) -> Self {
        Self { entries: Mutex::new(vec![(key, checkpoint)]), ..Self::default() }
    }

    fn failing_load() -> Self {
        Self { fail_load: true, ..Self::default() }
    }

    fn failing_save() -> Self {
        Self { fail_save: true, ..Self::default() }
    }

    fn saved(&self) -> Vec<(MailScanCheckpointKey, MailScanCheckpoint)> {
        self.saved.lock().unwrap().clone()
    }
}

impl MailScanCheckpointStore for FakeStore {
    fn load<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
    ) -> MailScanCheckpointFuture<'a, Option<MailScanCheckpoint>> {
        Box::pin(async move {
            if self.fail_load {
                return Err(ApplicationError::from(RepositoryError::Failed(
                    "fixture load failure".to_owned(),
                )));
            }
            let entries = self.entries.lock().unwrap();
            Ok(entries.iter().find(|(stored, _)| stored == key).map(|(_, value)| value.clone()))
        })
    }

    fn save<'a>(
        &'a self,
        key: &'a MailScanCheckpointKey,
        checkpoint: &'a MailScanCheckpoint,
    ) -> MailScanCheckpointFuture<'a, ()> {
        Box::pin(async move {
            if self.fail_save {
                return Err(ApplicationError::from(RepositoryError::Failed(
                    "fixture save failure".to_owned(),
                )));
            }
            let entry = (key.clone(), checkpoint.clone());
            self.saved.lock().unwrap().push(entry.clone());
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|(stored, _)| stored != key);
            entries.push(entry);
            Ok(())
        })
    }
}

fn client(endpoint: &str) -> MoogleMailClient {
    MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: Url::parse(endpoint).unwrap(),
        maximum_response_bytes: 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(1),
    })
    .unwrap()
}

fn make_sender(store: Arc<FakeStore>, endpoint: &str) -> MoogleMailSender {
    MoogleMailSender::new(client(endpoint)).with_mail_scan_checkpoint_store(store)
}

fn cookie(credential: u128, version: i64) -> MoogleMailCookie {
    let cookie = MoogleMailCookie::new("fixture-cookie").unwrap();
    cookie.bind_persisted_credential(Uuid::from_u128(credential), version);
    cookie
}

fn watermark(mid: &str) -> MailScanWatermark {
    MailScanWatermark {
        mail_id: mid.to_owned(),
        observed_at: Utc.with_ymd_and_hms(2026, 11, 4, 1, 0, 0).unwrap(),
    }
}

fn key(
    credential: u128,
    version: i64,
    stream: MailScanStream,
    endpoint: &str,
) -> MailScanCheckpointKey {
    MailScanCheckpointKey {
        credential_id: Uuid::from_u128(credential),
        credential_version: version,
        world_id: "main".to_owned(),
        stream,
        endpoint_scope: endpoint.to_owned(),
    }
}

#[tokio::test]
async fn sent_checkpoint_survives_restart_and_keeps_incremental_plan() {
    let store = Arc::new(FakeStore::empty());
    let first = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let first_cookie = cookie(9, 2);
    first
        .remember_sent_scan(&first_cookie, MoogleMailWorld::Main, Some(watermark("700")), true)
        .await;

    let saved = store.saved();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, key(9, 2, MailScanStream::Sent, "https://hentaiverse.org"));
    assert_eq!(saved[0].1.watermark.as_ref().unwrap().mail_id, "700");
    assert!(saved[0].1.last_full_scan_at.is_some());

    // 模拟新进程：新 sender 与新 Cookie 对象，只有持久凭据 ID/版本相同。
    let second = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let second_cookie = cookie(9, 2);
    second.ensure_sent_scan_checkpoint(&second_cookie, MoogleMailWorld::Main).await;
    let checkpoint = second.sent_scan_checkpoint(&second_cookie, MoogleMailWorld::Main).unwrap();
    assert_eq!(checkpoint.watermark.as_ref().unwrap().mail_id, "700");
    let plan = plan_mail_scan(Some(&checkpoint), Utc::now(), MAIL_SCAN_DAILY_FULL_INTERVAL);
    assert_eq!(plan.mode, MailScanMode::Incremental);
    assert_eq!(plan.reason, MailScanReason::Incremental);
}

#[tokio::test]
async fn read_mail_checkpoint_is_persisted_separately_from_sent() {
    let store = Arc::new(FakeStore::empty());
    let sender = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let cookie = cookie(9, 2);
    sender.remember_read_scan(&cookie, MoogleMailWorld::Main, Some(watermark("800")), false).await;

    let saved = store.saved();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0.stream, MailScanStream::ReadMail);
    assert!(saved[0].1.last_full_scan_at.is_none());

    // Read Mail 只绑定凭据，不借用 Sent 作用域；相同凭据仍能恢复。
    let restarted = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    restarted.ensure_read_scan_checkpoint(&cookie, MoogleMailWorld::Main).await;
    let checkpoint = restarted.read_scan_checkpoint(&cookie, MoogleMailWorld::Main).unwrap();
    assert_eq!(checkpoint.watermark.as_ref().unwrap().mail_id, "800");
}

#[tokio::test]
async fn checkpoint_is_isolated_by_credential_version_world_and_endpoint() {
    let valid = key(9, 2, MailScanStream::Sent, "https://hentaiverse.org");
    let checkpoint = MailScanCheckpoint {
        watermark: Some(watermark("700")),
        last_full_scan_at: Some(Utc.with_ymd_and_hms(2026, 11, 4, 0, 0, 0).unwrap()),
    };
    let store = Arc::new(FakeStore::with_checkpoint(valid.clone(), checkpoint));

    // 另一凭据 ID。
    let sender = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let other_credential = cookie(10, 2);
    sender.ensure_sent_scan_checkpoint(&other_credential, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&other_credential, MoogleMailWorld::Main).is_none());

    // 同一凭据的新版本（停用/改名后重新启用）。
    let bumped_version = cookie(9, 3);
    sender.ensure_sent_scan_checkpoint(&bumped_version, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&bumped_version, MoogleMailWorld::Main).is_none());

    // 另一世界。
    let same_credential = cookie(9, 2);
    sender.ensure_sent_scan_checkpoint(&same_credential, MoogleMailWorld::Isekai).await;
    assert!(sender.sent_scan_checkpoint(&same_credential, MoogleMailWorld::Isekai).is_none());

    // 另一 endpoint origin。
    let other_endpoint = make_sender(Arc::clone(&store), "http://127.0.0.1:9");
    other_endpoint.ensure_sent_scan_checkpoint(&same_credential, MoogleMailWorld::Main).await;
    assert!(other_endpoint.sent_scan_checkpoint(&same_credential, MoogleMailWorld::Main).is_none());

    // 原作用域仍可读取，证明隔离不是把存储整体禁用。
    sender.ensure_sent_scan_checkpoint(&same_credential, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&same_credential, MoogleMailWorld::Main).is_some());
}

#[tokio::test]
async fn invalid_persisted_checkpoint_falls_back_to_full_scan() {
    let store = Arc::new(FakeStore::with_checkpoint(
        key(9, 2, MailScanStream::Sent, "https://hentaiverse.org"),
        MailScanCheckpoint {
            // 前导零不是规范 MID；损坏行不能驱动增量前缀。
            watermark: Some(MailScanWatermark {
                mail_id: "007".to_owned(),
                observed_at: Utc.with_ymd_and_hms(2026, 11, 4, 1, 0, 0).unwrap(),
            }),
            last_full_scan_at: Some(Utc.with_ymd_and_hms(2026, 11, 4, 0, 0, 0).unwrap()),
        },
    ));
    let sender = make_sender(store, "https://hentaiverse.org");
    let cookie = cookie(9, 2);
    sender.ensure_sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).is_none());
    let plan = plan_mail_scan(None, Utc::now(), MAIL_SCAN_DAILY_FULL_INTERVAL);
    assert_eq!(plan, MailScanPlan::full(MailScanReason::NoCheckpoint));
}

#[tokio::test]
async fn load_and_save_failures_never_block_or_lose_current_result() {
    let store = Arc::new(FakeStore::failing_load());
    let sender = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let cookie = cookie(9, 2);
    sender.ensure_sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).is_none());

    // 持久化失败后，本轮已确认的水位仍留在内存，发送/维护结果不受影响。
    let failing_save = Arc::new(FakeStore::failing_save());
    let sender = make_sender(Arc::clone(&failing_save), "https://hentaiverse.org");
    sender.remember_sent_scan(&cookie, MoogleMailWorld::Main, Some(watermark("900")), true).await;
    let checkpoint = sender.sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).unwrap();
    assert_eq!(checkpoint.watermark.as_ref().unwrap().mail_id, "900");
    assert!(failing_save.saved().is_empty());
}

#[tokio::test]
async fn config_cookie_without_persistent_identity_stays_in_memory() {
    let store = Arc::new(FakeStore::empty());
    let sender = make_sender(Arc::clone(&store), "https://hentaiverse.org");
    let cookie = MoogleMailCookie::new("fixture-cookie").unwrap();
    sender.remember_sent_scan(&cookie, MoogleMailWorld::Main, Some(watermark("950")), true).await;
    // 内存水位仍生效，但配置 Cookie 没有稳定凭据 ID，不能写入数据库作用域。
    assert!(sender.sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).is_some());
    assert!(store.saved().is_empty());
    sender.ensure_sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).await;
    assert!(sender.sent_scan_checkpoint(&cookie, MoogleMailWorld::Main).is_some());
}

/// 常规轮命中水位锚点后只读前缀，不请求下一页；这不是完整枚举。
#[tokio::test]
async fn sent_incremental_prefix_stops_at_watermark_anchor_without_full_list()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let page = include_str!("../../../../tests/fixtures/w6_3/sent-main.html").to_owned();
    let served_page = page.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut request = Vec::new();
        loop {
            let mut buffer = [0; 1024];
            let length = stream.read(&mut buffer).await?;
            if length == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            request.extend_from_slice(&buffer[..length]);
            if request.windows(4).any(|value| value == b"\r\n\r\n") {
                break;
            }
        }
        let expected = "GET /?s=Bazaar&ss=mm&filter=sent HTTP/1.1\r\n";
        if !request.starts_with(expected.as_bytes()) {
            return Err(std::io::Error::other("Sent 前缀读取出现非预期请求"));
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{served_page}",
            served_page.len()
        );
        stream.write_all(response.as_bytes()).await?;
        Ok::<_, std::io::Error>(())
    });

    let parsed = MoogleMailParser.scan_sent_page(
        page.as_bytes(),
        MoogleMailWorld::Main,
        0,
        &mut std::collections::HashSet::new(),
    )?;
    // 不选最后一页最后一行：锚点之后仍需有更旧行，前缀才能在本页确认边界并停止。
    let anchor = parsed.summaries.get(5).ok_or("fixture Sent 页摘要不足")?;
    let endpoint = format!("http://{address}");
    let store = Arc::new(FakeStore::with_checkpoint(
        key(9, 2, MailScanStream::Sent, &endpoint),
        MailScanCheckpoint {
            watermark: Some(MailScanWatermark {
                mail_id: anchor.external_mail_id.clone(),
                observed_at: anchor.sent_at,
            }),
            last_full_scan_at: Some(Utc::now()),
        },
    ));
    let sender = make_sender(store, &endpoint);
    let scan = sender.scan_sent_summaries(&cookie(9, 2), MoogleMailWorld::Main).await?;
    server.await??;

    assert_eq!(scan.covered_from, Some(anchor.sent_at), "前缀必须报告覆盖起点");
    assert_eq!(scan.summaries.len(), 6, "锚点行及更新行都应返回，且不继续请求下一页");
    assert_eq!(
        scan.summaries.last().map(|entry| entry.external_mail_id.as_str()),
        Some(anchor.external_mail_id.as_str())
    );
    Ok(())
}

/// 持久锚点时间与当前列表不一致时必须回退完整扫描，不能用旧时间宣称覆盖。
#[test]
fn sent_prefix_with_changed_anchor_time_falls_back_to_full_scan()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let page = include_str!("../../../../tests/fixtures/w6_3/sent-main.html").to_owned();
        let served_page = page.clone();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = Arc::clone(&requests);
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let length = stream.read(&mut buffer).await?;
                    if length == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    request.extend_from_slice(&buffer[..length]);
                    if request.windows(4).any(|value| value == b"\r\n\r\n") {
                        break;
                    }
                }
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{served_page}",
                    served_page.len()
                );
                stream.write_all(response.as_bytes()).await?;
            }
            Ok::<_, std::io::Error>(())
        });

        let parsed = MoogleMailParser
            .scan_sent_page(page.as_bytes(), MoogleMailWorld::Main, 0, &mut std::collections::HashSet::new())?;
        let anchor = parsed.summaries.get(5).ok_or("fixture Sent 页摘要不足")?;
        let endpoint = format!("http://{address}");
        let store = Arc::new(FakeStore::with_checkpoint(
            key(9, 2, MailScanStream::Sent, &endpoint),
            MailScanCheckpoint {
                watermark: Some(MailScanWatermark {
                    mail_id: anchor.external_mail_id.clone(),
                    observed_at: anchor.sent_at + chrono::Duration::minutes(1),
                }),
                last_full_scan_at: Some(Utc::now()),
            },
        ));
        let sender = make_sender(store, &endpoint);
        let result = sender.scan_sent_summaries(&cookie(9, 2), MoogleMailWorld::Main).await;
        server.abort();
        assert!(result.is_err(), "锚点时间变化必须回退完整扫描并失败关闭");
        assert!(
            requests.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "必须实际发起完整扫描请求"
        );
        Ok(())
    })
}

/// 锚点正好在页末时，下一页同分钟的匹配也必须纳入；不能一见锚点就结束前缀。
#[test]
fn sent_prefix_carries_same_minute_rows_across_page_boundary()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let fixture = include_str!("../../../../tests/fixtures/w6_3/sent-main.html");
        let parsed = MoogleMailParser
            .scan_sent_page(fixture.as_bytes(), MoogleMailWorld::Main, 0, &mut std::collections::HashSet::new())?;
        let anchor = parsed.summaries.last().ok_or("fixture Sent 页没有摘要")?;
        let marker = format!("mid={}", anchor.external_mail_id);
        let marker_offset = fixture.find(&marker).ok_or("找不到锚点行")?;
        let row_start = fixture[..marker_offset].rfind("<tr").ok_or("找不到行起点")?;
        let row_end = marker_offset + fixture[marker_offset..].find("</tr>").ok_or("找不到行终点")? + 5;
        let row = fixture[row_start..row_end].replace(&marker, "mid=3125400");
        let table_start = fixture.find("<table id=\"mmail_list\">").ok_or("找不到表格")?;
        let table_header_end = table_start + fixture[table_start..].find("</tr>").ok_or("找不到表头")? + 5;
        let table_prefix = &fixture[table_start..table_header_end];
        let page_one = format!(
            "{table_prefix}{row}</table><div id=\"mmail_pager\"><div></div><div><span>Next ></span></div></div>"
        );

        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let responses = [fixture.to_owned(), page_one];
        let server = tokio::spawn(async move {
            for body in responses {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let length = stream.read(&mut buffer).await?;
                    if length == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    request.extend_from_slice(&buffer[..length]);
                    if request.windows(4).any(|value| value == b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await?;
            }
            Ok::<_, std::io::Error>(())
        });

        let endpoint = format!("http://{address}");
        let store = Arc::new(FakeStore::with_checkpoint(
            key(9, 2, MailScanStream::Sent, &endpoint),
            MailScanCheckpoint {
                watermark: Some(MailScanWatermark {
                    mail_id: anchor.external_mail_id.clone(),
                    observed_at: anchor.sent_at,
                }),
                last_full_scan_at: Some(Utc::now()),
            },
        ));
        let sender = make_sender(store, &endpoint);
        let scan = sender.scan_sent_summaries(&cookie(9, 2), MoogleMailWorld::Main).await?;
        server.await??;
        assert_eq!(scan.covered_from, Some(anchor.sent_at));
        assert_eq!(scan.summaries.len(), parsed.summaries.len() + 1);
        assert_eq!(scan.summaries.last().map(|entry| entry.external_mail_id.as_str()), Some("3125400"));
        Ok(())
    })
}

/// 同分钟内较低的 MID 出现在锚点之前是合法的，不能按数字顺序误判 AnchorMissing。
#[test]
fn sent_prefix_accepts_lower_mid_before_anchor_in_same_minute()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let fixture = include_str!("../../../../tests/fixtures/w6_3/sent-main.html");
        let parsed = MoogleMailParser
            .scan_sent_page(fixture.as_bytes(), MoogleMailWorld::Main, 0, &mut std::collections::HashSet::new())?;
        let anchor = parsed.summaries.get(5).ok_or("fixture Sent 页摘要不足")?;
        let marker = format!("mid={}", anchor.external_mail_id);
        let marker_offset = fixture.find(&marker).ok_or("找不到锚点行")?;
        let row_start = fixture[..marker_offset].rfind("<tr").ok_or("找不到行起点")?;
        let row_end = marker_offset + fixture[marker_offset..].find("</tr>").ok_or("找不到行终点")? + 5;
        let anchor_row = &fixture[row_start..row_end];
        let anchor_mid: u128 = anchor.external_mail_id.parse()?;
        let lower_mid = anchor_mid.checked_sub(1).ok_or("锚点 MID 无法减一")?;
        let lower_text = lower_mid.to_string();
        let lower_row = anchor_row.replace(&marker, &format!("mid={lower_text}"));
        let table_start = fixture.find("<table id=\"mmail_list\">").ok_or("找不到表格")?;
        let table_header_end = table_start + fixture[table_start..].find("</tr>").ok_or("找不到表头")? + 5;
        let table_prefix = &fixture[table_start..table_header_end];
        // 两行同一分钟，较小 MID 在锚点之前；页末没有下一页。
        let page_body = format!(
            "{table_prefix}{lower_row}{anchor_row}</table><div id=\"mmail_pager\"><div></div><div><span>Next ></span></div></div>"
        );

        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 1024];
                let length = stream.read(&mut buffer).await?;
                if length == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                request.extend_from_slice(&buffer[..length]);
                if request.windows(4).any(|value| value == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page_body}",
                page_body.len()
            );
            stream.write_all(response.as_bytes()).await?;
            Ok::<_, std::io::Error>(())
        });

        let endpoint = format!("http://{address}");
        let store = Arc::new(FakeStore::with_checkpoint(
            key(9, 2, MailScanStream::Sent, &endpoint),
            MailScanCheckpoint {
                watermark: Some(MailScanWatermark {
                    mail_id: anchor.external_mail_id.clone(),
                    observed_at: anchor.sent_at,
                }),
                last_full_scan_at: Some(Utc::now()),
            },
        ));
        let sender = make_sender(store, &endpoint);
        let scan = sender.scan_sent_summaries(&cookie(9, 2), MoogleMailWorld::Main).await?;
        server.await??;
        assert_eq!(scan.summaries.len(), 2);
        assert_eq!(scan.covered_from, Some(anchor.sent_at));
        assert!(lower_mid < anchor_mid, "测试行必须是更小的 MID");
        assert_eq!(scan.summaries[0].external_mail_id, lower_text);
        assert_eq!(scan.summaries[1].external_mail_id, anchor.external_mail_id);
        Ok(())
    })
}
