use reqwest::Client;
use url::Url;

/// E-Hentai 论坛适配器配置。认证与写入操作要等外部契约冻结后再实现。
pub struct EhentaiClient {
    client: Client,
    base_url: Url,
}

impl EhentaiClient {
    pub fn new(base_url: Url, client: Client) -> Self {
        Self { client, base_url }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }
}
