use super::*;

#[test]
fn probe_reports_listening_and_closed_ports() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let open = listener.local_addr().expect("local addr").port();
    assert!(probe_listener(open, Duration::from_millis(500)));
    let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    let closed_port = closed.local_addr().expect("local addr").port();
    drop(closed);
    assert!(!probe_listener(closed_port, Duration::from_millis(200)));
}
