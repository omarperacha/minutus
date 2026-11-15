//! Header-only capacity is a closed transport exception, never proof authority.
use super::*;
use alloy_consensus::Header;
use alloy_primitives::B256;
use std::{
    io::Write,
    net::{TcpListener, TcpStream},
    thread,
};

struct Server {
    port: u16,
    join: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(body: Vec<u8>, content_length: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let join = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            if read_request(&mut stream).is_err() {
                return;
            }
            let header = if content_length {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
            } else {
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".into()
            };
            if stream.write_all(header.as_bytes()).is_err() {
                return;
            }
            if !content_length {
                let _ = write!(stream, "{:x}\r\n", body.len());
            }
            let _ = stream.write_all(&body);
            if !content_length {
                let _ = stream.write_all(b"\r\n0\r\n\r\n");
            }
        });
        Ok(Self {
            port,
            join: Some(join),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn read_request(stream: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end])?;
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .ok_or("Missing request size")?;
            if bytes.len() >= end + 4 + length {
                return Ok(());
            }
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer)?;
        if count == 0 || bytes.len() + count > 16 * 1024 {
            return Err("Invalid fixture request".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
fn payload(count: usize) -> Result<(Header, Vec<u8>), Box<dyn std::error::Error>> {
    let header = Header {
        number: 26_000_000,
        gas_limit: 30_000_000,
        ..Header::default()
    };
    let mut value = serde_json::to_value(&header)?;
    value["hash"] = json!(format!("{:#x}", header.hash_slow()));
    value["transactions"] = json!(vec![format!("{:#x}", B256::repeat_byte(0x44)); count]);
    Ok((
        header,
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":1,"result":value}))?,
    ))
}

#[test]
fn valid_large_false_block_response_is_admitted_and_header_hash_still_verifies()
-> Result<(), Box<dyn std::error::Error>> {
    let (header, body) = payload(2000)?;
    assert!(body.len() > 64 * 1024 && body.len() < 512 * 1024);
    for method in ["eth_getBlockByNumber", "eth_getBlockByHash"] {
        let server = Server::new(body.clone(), true)?;
        let endpoint = EvmRpcUrl::loopback_test(server.port);
        let rpc = RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?;
        let selector = if method == "eth_getBlockByHash" {
            format!("{:#x}", header.hash_slow())
        } else {
            format!("0x{:x}", header.number)
        };
        let result = rpc
            .value_result_with_timeout(method, &json!([selector, false]), Duration::from_secs(1))
            .map_err(|error| format!("{error:?}"))?;
        let decoded: Header = serde_json::from_value(result.clone())?;
        assert_eq!(decoded.hash_slow(), header.hash_slow());
        assert_eq!(result["transactions"].as_array().map(Vec::len), Some(2000));
    }
    Ok(())
}

#[test]
fn ordinary_calls_full_blocks_and_nonexact_shapes_keep_sixty_four_kib()
-> Result<(), Box<dyn std::error::Error>> {
    let (_, body) = payload(2000)?;
    for (method, params) in [
        ("eth_call", json!([{}, "latest"])),
        ("eth_getProof", json!(["address", [], "latest"])),
        ("eth_getBlockByNumber", json!(["latest", true])),
        ("eth_getBlockByHash", json!(["latest", false, "extra"])),
        ("eth_getBlockByNumber", json!(["latest", "false"])),
    ] {
        let server = Server::new(body.clone(), true)?;
        let endpoint = EvmRpcUrl::loopback_test(server.port);
        let rpc = RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            rpc.value_result_with_timeout(method, &params, Duration::from_secs(1)),
            Err(RpcError::ResponseTooLarge)
        );
    }
    Ok(())
}

#[test]
fn headers_above_fixed_cap_reject_known_and_streamed_lengths()
-> Result<(), Box<dyn std::error::Error>> {
    let (_, body) = payload(8000)?;
    assert!(body.len() > 512 * 1024);
    for content_length in [true, false] {
        let server = Server::new(body.clone(), content_length)?;
        let endpoint = EvmRpcUrl::loopback_test(server.port);
        let rpc = RpcClient::new(&endpoint).map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            rpc.value_result_with_timeout(
                "eth_getBlockByNumber",
                &json!(["latest", false]),
                Duration::from_secs(1)
            ),
            Err(RpcError::ResponseTooLarge)
        );
    }
    Ok(())
}
