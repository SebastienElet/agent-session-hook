// Open choice: libcurl via curl/http; the alternative is ureq with rustls.
use crate::{Error, failure, webhook_format::Encoded};
use std::time::Duration;

pub(crate) fn send(url: &str, encoded: &Encoded) -> Result<(), Error> {
    let uri: http::Uri = url.parse().map_err(|_| failure("invalid webhook URL"))?;
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
    let mut headers = curl::easy::List::new();
    let mut request = curl::easy::Easy::new();
    let result = (|| {
        headers.append(&format!("Authorization: {}", encoded.authorization))?;
        headers.append("Content-Type: application/json")?;
        request.url(url)?;
        request.proxy("")?;
        request.follow_location(false)?;
        request.timeout(Duration::from_secs(3))?;
        request.connect_timeout(Duration::from_secs(3))?;
        request.post_fields_copy(&encoded.body)?;
        request.http_headers(headers)?;
        request.write_function(|bytes| Ok(bytes.len()))?;
        request.perform()?;
        request.response_code()
    })()
    .map_err(|_: curl::Error| failure("webhook delivery failed"))?;
    if !(200..300).contains(&result) {
        return Err(failure("webhook rejected delivery"));
    }
    Ok(())
}
