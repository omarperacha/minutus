use super::*;
use recovery_flow::{is_direct_call_batch, is_direct_discovery_call_batch};

fn call(id: &str, name: &str) -> ParsedToolCall {
    ParsedToolCall {
        call_id: id.into(),
        recipient: name.into(),
        name: name.into(),
        arguments: BTreeMap::new(),
    }
}

#[test]
fn standalone_transfer_bypasses_plan_deferral_without_becoming_read_only_discovery() {
    let transfer = call("transfer", "wallet.request_transfer");
    assert!(is_direct_call_batch(std::slice::from_ref(&transfer)));
    assert!(!is_direct_discovery_call_batch(std::slice::from_ref(
        &transfer
    )));
    assert!(!is_direct_call_batch(&[]));
    for sibling in [
        call("duplicate", "wallet.request_transfer"),
        call("web", "web.retrieve"),
        call("write", "filesystem.write_text"),
    ] {
        assert!(!is_direct_call_batch(&[transfer.clone(), sibling]));
    }
}
