//! 原生外部装备元数据的读取、名称绑定与拒绝边界回归。

use super::*;
use crate::clients::mooglemail::MoogleMailClientConfig;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const REFERENCE: &str = "/dynjs/1793537462/fixture/equip-77.js";

fn inventory_page(label: &str) -> String {
    format!(
        r#"<html><head><script>var MAIN_URL = "https://hentaiverse.org/";</script>
<script src="{REFERENCE}" type="text/javascript"></script></head>
<body><div id="mainpane"><div id="armory_outer">
<div id="equipcount">Selected 0 of 1 matching equipment available to organize</div>
<form id="equipform" method="post"><input name="postoken" type="hidden" value="fixture-token">
<table id="equiplist"><tbody><tr onclick="return select_equip(314,event,this)" onmouseover="hover_equip(314)">
<td><label for="e314"><input type="checkbox" name="eqids[]" id="e314" value="314"><span></span>{label}</label></td>
</tr></tbody></table></form></div></div></body></html>"#
    )
}

fn request() -> MoogleMailSendRequest {
    MoogleMailSendRequest {
        world: MoogleMailWorld::Main,
        message: MoogleMailMessageFields {
            recipient: "fixture-recipient".to_owned(),
            subject: "fixture-subject".to_owned(),
            body: "fixture-body".to_owned(),
        },
        credits: 0,
        cod: 0,
        equipment_ids: vec!["314".to_owned()],
    }
}

async fn fixture(
    page: String,
    content_type: &str,
    metadata: String,
) -> Result<
    (MoogleMailSender, JoinHandle<std::io::Result<()>>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let address = listener.local_addr()?;
    let content_type = content_type.to_owned();
    let server = tokio::spawn(async move {
        for (path, media_type, body) in [
            ("/?s=Bazaar&ss=am&screen=organize&filter=armor_cloth", "text/html".to_owned(), page),
            (REFERENCE, content_type, metadata),
        ] {
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
            let expected = format!("GET {path} HTTP/1.1\r\n");
            if !request.starts_with(expected.as_bytes()) {
                return Err(std::io::Error::other("读取元数据时出现非预期请求"));
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {media_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await?;
        }
        Ok(())
    });
    let client = MoogleMailClient::with_config(MoogleMailClientConfig {
        endpoint: Url::parse(&format!("http://{address}"))?,
        maximum_response_bytes: 8 * 1024 * 1024,
        minimum_request_interval: Duration::from_millis(1),
        request_timeout: Duration::from_secs(2),
    })?;
    Ok((MoogleMailSender::new(client), server))
}

#[tokio::test]
async fn inventory_metadata_binds_external_name_before_preparing_equipment() -> TestResult {
    // 名称中的锁图标不是状态；脚本说明中的登录/战斗标记也不是页面状态。
    let name = "Custom \u{1f512} Blade";
    let label = format!("\u{1f4e6} \u{1f6e1}\u{fe0f} {name}");
    let metadata = format!(
        "var dynjs_equip = {};",
        serde_json::json!({"314": {"t": name, "d": "you are not logged in <div id='battle_main'>"}})
    );
    let (sender, server) =
        fixture(inventory_page(&label), "application/javascript", metadata).await?;
    let cookie = MoogleMailCookie::new("fixture-cookie")?;
    let state = sender
        .read_inventory_preparation(&cookie, &request(), MoogleMailInventoryCategory::Cloth, |_| {
            true
        })
        .await?;
    server.await??;
    assert_eq!(state.equipment.len(), 1);
    let equipment = &state.equipment[0];
    assert_eq!(equipment.equipment_id, "314");
    assert!(equipment.stored && equipment.protected);
    assert!(!equipment.locked && !equipment.equipped);
    assert!(cookie.is_active());
    Ok(())
}

#[tokio::test]
async fn inventory_metadata_rejects_incomplete_or_ambiguous_names() -> TestResult {
    for metadata in [
        r#"var dynjs_equip = {};"#,
        r#"var dynjs_eqstore = {"314":{"t":"Fixture"}};"#,
        r#"var dynjs_equip = {"314":{"t":"Wrong"}};"#,
        r#"var dynjs_equip = {"314":{"t":"Fixture"},"314":{"t":"Fixture"}};"#,
        r#"var dynjs_equip = {"314":{"t":"Fixture","t":"Fixture"}};"#,
        r#"var dynjs_equip = {"314":{"t":"Fixture"}}; var dynjs_equip = {};"#,
        r#"var dynjs_equip = (() => ({"314":{"t":"Fixture"}}))();"#,
    ] {
        let (sender, server) =
            fixture(inventory_page("Fixture"), "text/javascript", metadata.to_owned()).await?;
        let cookie = MoogleMailCookie::new("fixture-cookie")?;
        let result = sender
            .read_inventory_preparation(
                &cookie,
                &request(),
                MoogleMailInventoryCategory::Cloth,
                |_| true,
            )
            .await;
        server.await??;
        assert!(
            matches!(result, Err(error) if error.category == MoogleMailErrorCategory::ProtocolDrift && !error.write_started())
        );
        assert!(cookie.is_active());
    }
    Ok(())
}

#[test]
fn inventory_metadata_rejects_non_native_label_decoration() -> TestResult {
    let ids = vec!["314".to_owned()];
    for decoration in [
        "<span hidden></span>",
        "<span>hidden state</span>",
        "<span></span><span></span>",
        "<span><span></span></span>",
    ] {
        let page = inventory_page("Fixture").replace("<span></span>", decoration);
        let result = MoogleMailParser.parse_inventory_preparation(
            page.as_bytes(),
            MoogleMailWorld::Main,
            MoogleMailInventoryCategory::Cloth,
            &ids,
        );
        assert!(
            matches!(result, Err(error) if error.category == MoogleMailErrorCategory::ProtocolDrift)
        );
    }
    Ok(())
}

#[tokio::test]
async fn inventory_metadata_rejects_html_responses() -> TestResult {
    for (media_type, metadata) in [
        ("text/html", r#"var dynjs_equip = {"314":{"t":"Fixture"}};"#),
        ("application/javascript", " \n<html><body>Fixture</body></html>"),
    ] {
        let (sender, server) =
            fixture(inventory_page("Fixture"), media_type, metadata.to_owned()).await?;
        let cookie = MoogleMailCookie::new("fixture-cookie")?;
        let result = sender
            .read_inventory_preparation(
                &cookie,
                &request(),
                MoogleMailInventoryCategory::Cloth,
                |_| true,
            )
            .await;
        server.await??;
        assert!(
            matches!(result, Err(error) if error.category == MoogleMailErrorCategory::ProtocolDrift && !error.write_started())
        );
    }
    Ok(())
}

#[test]
fn inventory_metadata_rejects_origin_world_and_path_substitution() -> TestResult {
    let client = MoogleMailClient::production()?;
    for source in [
        "https://other.invalid/dynjs/77/fixture/equip-77.js",
        "https://user@hentaiverse.org/dynjs/77/fixture/equip-77.js",
        "/isekai/dynjs/77/fixture/equip-77.js",
        "/dynjs/077/fixture/equip-077.js",
        "/dynjs/77/fixture/equip-0.js",
        "/dynjs/77/fixture/other.js",
        "/dynjs/77/fixture/../fixture/equip-77.js",
        "/dynjs/77/fixture/%65quip-77.js",
        "/dynjs/77/fixture/equip-77.js?revision=1",
        "/dynjs/77/fixture/equip-77.js#fragment",
        "//other.invalid/dynjs/77/fixture/equip-77.js",
    ] {
        assert!(client.inventory_metadata(MoogleMailWorld::Main, source).is_err());
    }
    assert!(client.inventory_metadata(MoogleMailWorld::Isekai, REFERENCE).is_err());
    Ok(())
}
