//! JSON-RPC member presence is distinct from a legitimate null result.
use super::*;

#[test]
fn null_results_are_present_but_missing_ambiguous_or_invalid_envelopes_reject() {
    let null = decode_response(br#"{"jsonrpc":"2.0","id":1,"result":null}"#)
        .and_then(|response| response.result.ok_or(RpcError::MalformedResponse));
    assert_eq!(null, Ok(Value::Null));
    for invalid in [
        br#"{"jsonrpc":"2.0","id":1}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":1,"result":null,"error":{"code":-32000}}"#,
        br#"{"jsonrpc":"2.0","id":1,"result":null,"error":null}"#,
        br#"{"jsonrpc":"2.0","id":1,"error":null}"#,
        br#"{"jsonrpc":"2.0","id":2,"result":null}"#,
        br#"{"jsonrpc":"2.0","id":1,"result":null,"result":7}"#,
        br#"{"jsonrpc":"1.0","id":1,"result":null}"#,
    ] {
        assert_eq!(
            decode_response(invalid).map(|_| ()),
            Err(RpcError::MalformedResponse)
        );
    }
}
