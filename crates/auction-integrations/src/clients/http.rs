/// 构建统一使用 rustls/ring 的外部 HTTP 客户端。
///
/// 外部适配器应复用该客户端，避免每个网关各自维护连接池或选择不同的 TLS provider。
pub fn build_http_client() -> Result<reqwest::Client, reqwest::Error> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder().build()
}
