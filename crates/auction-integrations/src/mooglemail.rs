use reqwest::Client;
use url::Url;

/// MoogleMail adapter configuration. Delivery semantics belong to the
/// application port; this crate only owns transport and authentication code.
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
