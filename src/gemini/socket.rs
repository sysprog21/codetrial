//! Opens the Gemini Live socket, through a proxy when the environment names
//! one. reqwest already honors `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` for
//! every HTTP call; this applies the same rules through the same matcher and
//! CONNECT tunnel. Otherwise, on a network that only lets traffic out through a
//! proxy, the report would work and the live interview would not.
//!
//! Only `wss://` is proxied, as reqwest proxies only `https://` through
//! `HTTPS_PROXY`. A plain `ws://` URL is a loopback fixture, which a proxy
//! configured for the machine would receive and usually fail.

use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::connect::proxy::Tunnel;
use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, client_async_tls_with_config,
    connect_async_tls_with_config,
};
use tower_service::Service;

pub(super) type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(super) async fn connect(url: &str) -> Result<Socket, BoxError> {
    let request = url.into_client_request()?;
    let connector = Some(Connector::Rustls(
        std::sync::Arc::clone(crate::tls_config()),
    ));
    #[cfg(test)]
    if let Some(stream) = loopback::connect(url).await? {
        let (socket, _) = client_async_tls_with_config(request, stream, None, connector).await?;
        return Ok(socket);
    }
    let Some(proxy) = proxy_for(request.uri(), &Matcher::from_env()) else {
        let (socket, _) = connect_async_tls_with_config(request, None, false, connector).await?;
        return Ok(socket);
    };
    let stream = tunnel(&proxy, request.uri()).await?;
    let (socket, _) = client_async_tls_with_config(request, stream, None, connector).await?;
    Ok(socket)
}

fn proxy_for(uri: &Uri, proxies: &Matcher) -> Option<Intercept> {
    if uri.scheme_str() != Some("wss") {
        return None;
    }
    // The matcher keys on the HTTP scheme the socket upgrades from.
    let https = Uri::builder()
        .scheme("https")
        .authority(uri.authority()?.clone())
        .path_and_query("/")
        .build()
        .ok()?;
    proxies.intercept(&https)
}

/// A TCP stream tunneled with CONNECT through `proxy` to `target`'s host,
/// ready for TLS. Only an `http://` proxy, as reqwest here is built without
/// SOCKS support, so a SOCKS proxy would fail every HTTP call anyway.
async fn tunnel(proxy: &Intercept, target: &Uri) -> Result<TcpStream, BoxError> {
    let uri = proxy.uri();
    if uri.scheme_str() != Some("http") {
        return Err(format!(
            "the Gemini Live socket goes through an http:// proxy, not {}://",
            uri.scheme_str().unwrap_or("")
        )
        .into());
    }
    let mut connector = Tunnel::new(uri.clone(), HttpConnector::new());
    if let Some(auth) = proxy.basic_auth() {
        connector = connector.with_auth(auth.clone());
    }
    Ok(connector.call(target.clone()).await?.into_inner())
}

#[cfg(test)]
#[path = "../../tests/unit/gemini/socket.rs"]
mod tests;

#[cfg(test)]
#[path = "socket/loopback.rs"]
pub(super) mod loopback;
