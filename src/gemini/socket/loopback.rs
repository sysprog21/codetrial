//! A scoped socket-option fixture, retaining the real TCP and WebSocket path.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpSocket, TcpStream};

static ACTIVE: Mutex<Option<Arc<State>>> = Mutex::new(None);
const REQUESTED: u32 = 1024;
const MAX_BUFFER: u32 = 16384;

struct State {
    endpoint: String,
    address: SocketAddrV4,
    claimed: AtomicBool,
    buffers: Mutex<Option<(u32, u32)>>,
}

/// Owned by one isolated fixture until its real session and peer are joined.
pub(crate) struct Registration {
    state: Arc<State>,
}

pub(in crate::gemini) fn install(endpoint: &str) -> io::Result<Registration> {
    let parsed = reqwest::Url::parse(endpoint).map_err(io::Error::other)?;
    if parsed.scheme() != "ws"
        || parsed.host_str() != Some("127.0.0.1")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query() != Some("key=sequence-test-key")
        || parsed.fragment().is_some()
        || parsed.as_str() != endpoint
        || parsed.port().is_none_or(|port| port == 0)
    {
        return Err(io::Error::other(
            "socket fixture requires its exact authored loopback URL",
        ));
    }
    let state = Arc::new(State {
        endpoint: parsed.to_string(),
        address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, parsed.port().unwrap_or(0)),
        claimed: AtomicBool::new(false),
        buffers: Mutex::new(None),
    });
    let mut active = ACTIVE.lock().unwrap_or_else(|error| error.into_inner());
    if active.is_some() {
        return Err(io::Error::other("a socket fixture is already installed"));
    }
    *active = Some(Arc::clone(&state));
    Ok(Registration { state })
}

fn claim(endpoint: &str) -> io::Result<Option<Arc<State>>> {
    let active = ACTIVE.lock().unwrap_or_else(|error| error.into_inner());
    let Some(state) = active.as_ref().filter(|state| state.endpoint == endpoint) else {
        return Ok(None);
    };
    if state.claimed.swap(true, Ordering::SeqCst) {
        return Err(io::Error::other("the socket fixture was already consumed"));
    }
    Ok(Some(Arc::clone(state)))
}

fn admitted(bytes: u32) -> io::Result<u32> {
    if (1..=MAX_BUFFER).contains(&bytes) {
        Ok(bytes)
    } else {
        Err(io::Error::other(
            "socket buffer exceeds the declared fixture bound",
        ))
    }
}

pub(super) async fn connect(endpoint: &str) -> io::Result<Option<TcpStream>> {
    let Some(state) = claim(endpoint)? else {
        return Ok(None);
    };
    let socket = TcpSocket::new_v4()?;
    socket.set_send_buffer_size(REQUESTED)?;
    let before = admitted(socket.send_buffer_size()?)?;
    let stream = socket.connect(state.address.into()).await?;
    let connected = admitted(
        u32::try_from(socket2::SockRef::from(&stream).send_buffer_size()?)
            .map_err(io::Error::other)?,
    )?;
    *state
        .buffers
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((before, connected));
    Ok(Some(stream))
}

impl Registration {
    pub(crate) fn sender_buffers(&self) -> Option<(u32, u32)> {
        *self
            .state
            .buffers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(|error| error.into_inner());
        if active
            .as_ref()
            .is_some_and(|state| Arc::ptr_eq(state, &self.state))
        {
            *active = None;
        }
    }
}

#[path = "../../../tests/unit/gemini/socket_loopback.rs"]
mod tests;
