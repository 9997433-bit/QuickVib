//! Socket setup failures must not be mislabeled as allow-list refusals.

use quickvib_device::listener::Refusal;

#[test]
fn socket_setup_failure_has_a_distinct_refusal_reason() {
    assert_ne!(Refusal::SocketError, Refusal::PeerNotAllowed);
    assert_ne!(Refusal::SocketError, Refusal::AlreadyConnected);
}
