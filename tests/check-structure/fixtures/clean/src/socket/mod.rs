mod datagram;

pub use datagram::send;

pub fn send_now() -> crate::kinds::Ready {
    send()
}
