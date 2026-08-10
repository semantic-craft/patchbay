/// Blocking HTTP client honouring the user's configured proxy.
///
/// The only remaining network caller is the app-update check, so this stays a
/// single builder rather than a client abstraction.
pub fn build_http_client(proxy_url: Option<&str>, timeout_secs: u64) -> reqwest::blocking::Client {
    let mut builder = reqwest::blocking::Client::builder()
        .user_agent("patchbay")
        .timeout(std::time::Duration::from_secs(timeout_secs));
    if let Some(proxy) = proxy_url.filter(|s| !s.is_empty()) {
        if let Ok(p) = reqwest::Proxy::all(proxy) {
            builder = builder.proxy(p);
        }
    }
    builder.build().unwrap_or_default()
}
