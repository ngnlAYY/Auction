use reqwest::Client;
use url::Url;

/// E-Hentai forum adapter configuration. Authentication and write operations
/// remain intentionally unimplemented until the external contract is frozen.
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
