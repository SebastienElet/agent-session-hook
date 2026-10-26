use crate::{Error, failure, webhook_format::Encoded};
use std::time::Duration;

pub(crate) fn send(url: &str, encoded: &Encoded) -> Result<(), Error> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| failure("invalid webhook URL"))?;
    let local_http =
        uri.scheme_str() == Some("http") && matches!(uri.host(), Some("127.0.0.1" | "[::1]"));
    if (uri.scheme_str() != Some("https") && !local_http)
        || uri
            .authority()
            .is_none_or(|authority| authority.as_str().contains('@'))
        || url.contains('#')
    {
        return Err(failure(
            "webhook requires HTTPS or loopback HTTP without userinfo or fragment",
        ));
    }
    let tls = ureq::tls::TlsConfig::builder()
        .unversioned_rustls_crypto_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .build();
    let agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .new_agent();
    let mut response = agent
        .post(url)
        .header("Authorization", &encoded.authorization)
        .header("Content-Type", "application/json")
        .send(encoded.body.as_slice())
        .map_err(|_| failure("webhook delivery failed"))?;
    std::io::copy(&mut response.body_mut().as_reader(), &mut std::io::sink())
        .map_err(|_| failure("webhook delivery failed"))?;
    if !response.status().is_success() {
        return Err(failure("webhook rejected delivery"));
    }
    Ok(())
}
