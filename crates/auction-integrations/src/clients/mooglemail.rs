use reqwest::Client;
use url::Url;

/// MoogleMail 适配器配置。投递语义属于应用端口；本 crate 只负责传输与认证代码。
pub struct MoogleMailClient {
    client: Client,
    base_url: Url,
}

impl MoogleMailClient {
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
