use std::time::Duration;

pub fn http_client() -> reqwest::Client {
    client_with_timeout(Duration::from_secs(15))
}

pub(crate) fn client_with_timeout(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("Failed to build HTTP client")
}
