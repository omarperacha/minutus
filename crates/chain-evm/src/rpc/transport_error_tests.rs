//! Error classification preserves the existing redacted public transport surface.
use super::*;
use std::{
    net::{TcpListener, TcpStream},
    thread,
    time::Instant,
};
#[test]
fn explicit_body_timeout_is_closed_and_other_errors_do_not_project_text() {
    assert_eq!(
        body_read_error(&io::Error::from(io::ErrorKind::TimedOut)),
        RpcError::TimedOut
    );
    let secret = io::Error::other("https://user:secret@private.invalid/key?token=credential");
    assert_eq!(body_read_error(&secret), RpcError::Transport);
    assert_eq!(
        RpcError::TimedOut.to_string(),
        RpcError::Transport.to_string()
    );
    assert!(!body_read_error(&secret).to_string().contains("credential"));
}
#[test]
fn bounded_local_request_timeout_is_distinct_before_public_transport_mapping()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match listener.accept() {
                Ok((_stream, _)) => {
                    thread::sleep(Duration::from_millis(150));
                    return;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(2));
                }
                _ => return,
            }
        }
    });
    let endpoint = EvmRpcUrl::loopback_test(port);
    let rpc = RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?;
    let result =
        rpc.value_result_with_timeout("eth_chainId", &json!([]), Duration::from_millis(20));
    // Unblock only this fixture's listener if the client timed out pre-connect.
    let _ = TcpStream::connect(("127.0.0.1", port));
    server.join().map_err(|_| "timeout fixture panicked")?;
    assert_eq!(result, Err(RpcError::TimedOut));
    Ok(())
}
