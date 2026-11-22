Vendored from [`livekit-net` 0.2.0](https://crates.io/crates/livekit-net),
upstream revision `c0de3bc4c1c129b1e5a420f80bbffa0b4595396c`, `livekit-net/`.
Licensed under Apache-2.0; see LICENSE.

Two changes from upstream:

- The tokio-tungstenite dependency changes from 0.29 to 0.30, including its
  upstream tests. The companion tokio-tungstenite patch moves its bundled
  certificate roots to webpki-roots 1, fixing RUSTSEC-2026-0381 without
  changing either transport implementation.
- `src/native/proxy.rs` tunnels `wss://` through `HTTPS_PROXY` with the
  `rustls-tls-webpki-roots` feature too, trusting the webpki-roots 1 roots the
  direct path uses. Upstream refuses a proxied `wss://` connection unless
  `rustls-tls-native-roots` is enabled, so LiveKit signaling failed on any
  network that requires a proxy. `Cargo.toml` adds `tokio-rustls` and
  `webpki-roots` to that feature for it.

Remove the Cargo patch and this directory when upstream releases a network
transport using tokio-tungstenite 0.30 or later whose proxied `wss://` path
works with `rustls-tls-webpki-roots`.
