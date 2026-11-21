//! 按固定的三个装备请求参数回放指定 HTTP 状态、头和分块。

use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::Response;
use axum::routing::any;
use futures_util::StreamExt;
use futures_util::stream;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use url::Url;
use url::form_urlencoded;

use super::{FIXTURE_KEY, MISSING_PARAMETER, NOT_FOUND};

/// 一段响应数据或故意触发的连接中断。
#[derive(Clone)]
pub enum Chunk {
    /// 等待指定时长后送出字节。
    Data(Duration, Bytes),
    /// 等待指定时长后中断正文。
    Disconnect(Duration),
}

/// 对匹配请求使用的回放脚本。正文和头仅在脚本内持有。
#[derive(Clone)]
pub struct ResponseScript {
    /// 状态码。
    pub status: StatusCode,
    /// 有序分块。
    pub chunks: Vec<Chunk>,
    /// 响应头，可以用来设置内容编码、限流提示或跳转。
    pub headers: HeaderMap,
    /// 发送响应头前的延迟。
    pub initial_delay: Duration,
}

impl ResponseScript {
    /// 构造一段静态 JSON 正文。
    pub fn json(status: StatusCode, body: impl Into<Bytes>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
        Self {
            status,
            chunks: vec![Chunk::Data(Duration::ZERO, body.into())],
            headers,
            initial_delay: Duration::ZERO,
        }
    }
}

/// 每个请求仅保留断言所需的标志，避免把 URL、头值或凭证带入测试输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCheck {
    /// 方法与路径是 GET /equip。
    pub is_equipment_get: bool,
    /// 三个参数是依次编码的规范 EID、夹具 key 和世界布尔值。
    pub has_expected_query: bool,
    /// 是否出现了 Cookie、Authorization 或 Referer。
    pub has_credentials_or_referer: bool,
}

struct DoubleState {
    equipment_id: String,
    isekai: bool,
    script: ResponseScript,
    requests: Mutex<Vec<RequestCheck>>,
}

/// 回环装备来源，销毁时中止监听。
pub struct EquipmentDouble {
    address: SocketAddr,
    state: Arc<DoubleState>,
    task: JoinHandle<()>,
}

impl EquipmentDouble {
    /// 仅监听 IPv4 回环临时端口；脚本只响应给定的虚构编号、夹具 key 和世界。
    pub async fn start(
        equipment_id: &str,
        isekai: bool,
        script: ResponseScript,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(DoubleState {
            equipment_id: equipment_id.to_owned(),
            isekai,
            script,
            requests: Mutex::new(Vec::new()),
        });
        let router = Router::new().fallback(any(handle)).with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { address, state, task })
    }

    /// 回环地址，供测试装配绑定 DNS 结果。
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// 回环装备入口，供其他测试装配使用。
    pub fn endpoint(&self) -> Result<Url, url::ParseError> {
        Url::parse(&format!("http://{}/equip", self.address))
    }

    /// 按到达顺序取得脱敏请求记录。
    pub fn requests(&self) -> Vec<RequestCheck> {
        self.state.requests.lock().expect("替身请求记录锁").clone()
    }
}

impl Drop for EquipmentDouble {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle(
    State(state): State<Arc<DoubleState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let is_equipment_get = method == Method::GET && uri.path() == "/equip";
    let expected_world = if state.isekai { "true" } else { "false" };
    let query = uri.query().unwrap_or_default();
    let has_expected_query = {
        let mut expected = form_urlencoded::Serializer::new(String::new());
        expected
            .append_pair("eid", &state.equipment_id)
            .append_pair("key", FIXTURE_KEY)
            .append_pair("is_isekai", expected_world);
        query == expected.finish()
    };
    let has_credentials_or_referer = [header::COOKIE, header::AUTHORIZATION, header::REFERER]
        .iter()
        .any(|name| headers.contains_key(name));
    state.requests.lock().expect("替身请求记录锁").push(RequestCheck {
        is_equipment_get,
        has_expected_query,
        has_credentials_or_referer,
    });

    let script = if is_equipment_get && has_expected_query {
        state.script.clone()
    } else {
        let pairs = form_urlencoded::parse(query.as_bytes()).collect::<Vec<_>>();
        let missing = ["eid", "key", "is_isekai"]
            .iter()
            .any(|name| !pairs.iter().any(|(key, _)| key == *name));
        if is_equipment_get && missing {
            ResponseScript::json(StatusCode::UNPROCESSABLE_ENTITY, MISSING_PARAMETER)
        } else {
            ResponseScript::json(StatusCode::NOT_FOUND, NOT_FOUND)
        }
    };
    tokio::time::sleep(script.initial_delay).await;
    let stream = stream::iter(script.chunks).then(async |chunk| match chunk {
        Chunk::Data(delay, bytes) => {
            tokio::time::sleep(delay).await;
            Ok(bytes)
        }
        Chunk::Disconnect(delay) => {
            tokio::time::sleep(delay).await;
            Err(io::Error::new(io::ErrorKind::ConnectionReset, "测试替身主动断开"))
        }
    });
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = script.status;
    *response.headers_mut() = script.headers;
    response
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};

    use super::{DoubleState, RequestCheck, ResponseScript, handle};
    use crate::hvdata::{EQUIPMENT, FIXTURE_EID, FIXTURE_KEY};

    #[tokio::test]
    async fn hvdata_double_routes_exact_parameters_and_records_only_flags() {
        let state = Arc::new(DoubleState {
            equipment_id: FIXTURE_EID.to_owned(),
            isekai: false,
            script: ResponseScript::json(StatusCode::OK, EQUIPMENT),
            requests: Mutex::new(Vec::new()),
        });
        let query = format!("/equip?eid={FIXTURE_EID}&key={FIXTURE_KEY}&is_isekai=false");
        for (method, target, status, is_equipment_get, has_expected_query) in [
            (Method::GET, query.clone(), StatusCode::OK, true, true),
            (
                Method::GET,
                query.replace(FIXTURE_KEY, "badbad0000"),
                StatusCode::NOT_FOUND,
                true,
                false,
            ),
            (
                Method::GET,
                format!("/equip?eid={FIXTURE_EID}&is_isekai=false"),
                StatusCode::UNPROCESSABLE_ENTITY,
                true,
                false,
            ),
            (Method::POST, query, StatusCode::NOT_FOUND, false, true),
        ] {
            let uri: Uri = target.parse().expect("替身测试地址");
            let response = handle(State(Arc::clone(&state)), method, uri, HeaderMap::new()).await;
            assert_eq!(response.status(), status);
            assert_eq!(
                state.requests.lock().expect("替身请求记录锁").last(),
                Some(&RequestCheck {
                    is_equipment_get,
                    has_expected_query,
                    has_credentials_or_referer: false,
                })
            );
        }
    }
}
