//! One fresh native consent for the exact outbound observation. This is not
//! transaction approval and provides no wallet or signing authority.
use super::intent::ObservationRequest;
use chain_evm::{EvmRpcUrl, contract::ContractBlock};
use ipc_types::IpcError;

#[cfg(target_os = "macos")]
pub(super) fn review(
    app: &tauri::AppHandle,
    request: &ObservationRequest,
    endpoint: &EvmRpcUrl,
) -> Result<bool, IpcError> {
    use objc2::{MainThreadMarker, rc::autoreleasepool};
    use objc2_app_kit::{
        NSAlert, NSAlertSecondButtonReturn, NSAutoresizingMaskOptions, NSScrollView, NSTextView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::sync::mpsc;

    let text = review_text(request, endpoint);
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let decision = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Allow unsigned contract observation?"));
            alert.setInformativeText(&NSString::from_str(
                "Review the provider, target, caller, exact calldata and any requested source/compiler origins below. This is one-time outbound permission, not transaction approval. No wallet unlock or signing.",
            ));
            alert.addButtonWithTitle(&NSString::from_str("Deny"));
            alert.addButtonWithTitle(&NSString::from_str("Allow once"));
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 220.0));
            let scroll = NSScrollView::initWithFrame(mtm.alloc(), frame);
            scroll.setHasVerticalScroller(true);
            let view = NSTextView::initWithFrame(mtm.alloc(), frame);
            view.setRichText(false);
            view.setEditable(false);
            view.setSelectable(true);
            view.setVerticallyResizable(true);
            view.setHorizontallyResizable(false);
            view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            view.setMinSize(NSSize::new(0.0, 220.0));
            view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
            // initWithFrame creates the default wrapping text container. Keep
            // horizontal resizing off so long URLs remain vertically scrollable.
            view.setString(&NSString::from_str(&text));
            scroll.setDocumentView(Some(&view));
            alert.setAccessoryView(Some(&scroll));
            Ok(alert.runModal() == NSAlertSecondButtonReturn)
        });
        let _ = sender.send(decision);
    })
    .map_err(|_| IpcError::internal())?;
    receiver.recv().map_err(|_| IpcError::internal())?
}

#[cfg(not(target_os = "macos"))]
pub(super) fn review(
    _app: &tauri::AppHandle,
    _request: &ObservationRequest,
    _endpoint: &EvmRpcUrl,
) -> Result<bool, IpcError> {
    Err(IpcError::permission_denied(
        "Complete native contract review is unavailable on this platform",
    ))
}

fn review_text(request: &ObservationRequest, endpoint: &EvmRpcUrl) -> String {
    let block = match &request.block {
        ContractBlock::Latest => "latest (resolved afresh)",
        ContractBlock::Hash(hash) => hash,
    };
    let call = request.call.as_ref().map_or_else(||"Runtime code metadata only; no calldata.".into(),|call|format!(
        "Simulated caller: {}\nFunction (unverified): {}\nArguments: {}\nExact calldata: {}\nValue: 0 wei; gas cap: 1,000,000",
        request.caller.as_deref().unwrap_or("0x0000000000000000000000000000000000000000"),call.signature(),crate::web_approval::review_json(&serde_json::Value::Array(call.arguments().to_vec())),call.calldata()));
    let invocation = request.invocation.as_ref().map_or_else(String::new, |local| {
        let intent = &local.intent;
        format!(
            "\n\nLOCAL HYPOTHETICAL INVOCATION — NOT SENT TO RPC\nCaller: {}\nOrigin: {}\nValue: {} wei\nFunction (source data): {}\nArguments: {}\nLocal calldata: {}\nThis context stays on this Mac. No caller account lookup, eth_call or callee fetch. Direct required callees remain unqualified; this grants no business scope or write authority.",
            intent.account(), local.origin, intent.value_wei(), intent.call().signature(),
            crate::web_approval::review_json(&serde_json::Value::Array(intent.call().arguments().to_vec())),
            intent.call().calldata()
        )
    });
    let source = if request.include_source {
        "\n\nSOURCE CORRESPONDENCE REQUESTED\nPublic source service: https://sourcify.dev\nOfficial compiler downloads: https://binaries.soliditylang.org\nThe source service receives only public chain/address lookups, including proof-derived implementation dependencies. Compiler downloads reveal compiler versions. Source is reproduced locally and compared with proof-checked runtime at the requested snapshot. No transaction calldata, signatures or wallet secrets are sent to these services. ABI/layout/source identifiers are returned only as unsigned metadata, with no docs/comments, write authority or economic-safety claim."
    } else {
        ""
    };
    let provider = if request.call.is_some() {
        "The provider sees your IP address and the displayed public addresses/call bytes. Arguments may contain sensitive data: deny if you do not intend to share them."
    } else {
        "The provider sees your IP address and public target/code-dependency addresses for chain/block/code checks. Local hypothetical invocation arguments, caller, origin, value and calldata are not sent to the provider."
    };
    format!(
        "Allow this one-time outbound observation?\n\nNetwork: {}\nProvider origin: {}\nTarget: {}\nBlock: {}\n{}{}{}\n\n{} This permission covers the displayed outbound call, chain/block/code checks and only the explicitly requested source/compiler acquisition above.\n\nNo wallet, password, signature or transaction. Simulated state changes are discarded. Caller-declared ABI and provider data remain untrusted; separately reproduced source correspondence does not establish economic safety or qualify effects.",
        request.network.descriptor().display_name(),
        endpoint.public_origin(),
        request.target,
        block,
        call,
        invocation,
        source,
        provider
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outbound_review_shows_exact_data_without_rpc_credentials()
    -> Result<(), Box<dyn std::error::Error>> {
        let call = chain_evm::contract::DeclaredContractCall::new(
            "0x1111111111111111111111111111111111111111",
            "f(bytes32)",
            &[serde_json::json!(format!("0x{}", "ab".repeat(32)))],
            &[],
        )?;
        let calldata = call.calldata().to_owned();
        let request = ObservationRequest {
            include_source: false,
            network: chain_evm::EvmNetwork::BaseSepolia,
            target: call.target().into(),
            block: ContractBlock::Latest,
            caller: None,
            call: Some(call),
            invocation: None,
        };
        let endpoint = EvmRpcUrl::new(
            "https://user:secret@provider.example/private-key?token=credential".into(),
        )?;
        let text = review_text(&request, &endpoint);
        assert!(text.contains("https://provider.example"));
        assert!(text.contains(&calldata));
        for secret in ["user:secret", "private-key", "credential"] {
            assert!(!text.contains(secret));
        }
        Ok(())
    }

    #[test]
    fn source_compiler_origins_require_the_explicit_source_mode()
    -> Result<(), Box<dyn std::error::Error>> {
        let endpoint =
            EvmRpcUrl::new("https://user:secret@provider.example/key?token=credential".into())?;
        let mut request = ObservationRequest {
            include_source: false,
            network: chain_evm::EvmNetwork::EthereumSepolia,
            target: "0x1111111111111111111111111111111111111111".into(),
            block: ContractBlock::Hash(format!("0x{}", "a".repeat(64))),
            call: None,
            caller: None,
            invocation: None,
        };
        let plain = review_text(&request, &endpoint);
        assert!(!plain.contains("https://sourcify.dev"));
        request.include_source = true;
        let source = review_text(&request, &endpoint);
        assert!(source.contains("https://sourcify.dev"));
        assert!(source.contains("https://binaries.soliditylang.org"));
        assert!(source.contains("No transaction calldata, signatures or wallet secrets"));
        assert!(source.contains(&format!("0x{}", "a".repeat(64))));
        for secret in ["user:secret", "token=credential", "/key?"] {
            assert!(!source.contains(secret));
        }
        Ok(())
    }
}

#[cfg(test)]
mod dynamic_text_tests {
    use super::*;
    #[test]
    fn nested_strings_escape_invisible_and_direction_controls()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = serde_json::json!([["safe\u{202e}evil\u{2066}x\u{200b}y\\u202e"]]);
        let call = chain_evm::contract::DeclaredContractCall::new(
            "0x1111111111111111111111111111111111111111",
            "f(string[])",
            original.as_array().ok_or("array")?,
            &[],
        )?;
        let request = ObservationRequest {
            include_source: false,
            network: chain_evm::EvmNetwork::EthereumMainnet,
            target: call.target().into(),
            block: ContractBlock::Latest,
            caller: None,
            call: Some(call),
            invocation: None,
        };
        let text = review_text(
            &request,
            &EvmRpcUrl::new("https://provider.example".into())?,
        );
        for control in ['\u{202e}', '\u{2066}', '\u{200b}'] {
            assert!(!text.contains(control));
        }
        assert!(text.contains("\\u202e"));
        assert!(text.contains("\\u2066"));
        assert!(text.contains("\\u200b"));
        assert!(text.contains(request.call.as_ref().ok_or("call")?.calldata()));
        Ok(())
    }

    #[test]
    fn hypothetical_context_is_local_only_and_cannot_spoof_the_native_disclosure()
    -> Result<(), Box<dyn std::error::Error>> {
        let call = model_runtime::ParsedToolCall {
            call_id: "local".into(),
            recipient: "contract.inspect".into(),
            name: "contract.inspect".into(),
            arguments: serde_json::from_value(serde_json::json!({
                "network":"ethereumSepolia", "block":"latest", "includeSource":true,
                "target":"0x1111111111111111111111111111111111111111",
                "invocation":{"function":"f(string)","arguments":["data\u{202e}spoof\u{200b}"],
                    "caller":"0x2222222222222222222222222222222222222222","valueWei":"7"}
            }))?,
        };
        let request = super::super::intent::request(&call).map_err(|error| error.message)?;
        let text = review_text(
            &request,
            &EvmRpcUrl::new("https://provider.example".into())?,
        );
        assert!(text.contains("LOCAL HYPOTHETICAL INVOCATION — NOT SENT TO RPC"));
        assert!(text.contains("Value: 7 wei"));
        assert!(text.contains("Origin: 0x2222222222222222222222222222222222222222"));
        assert!(text.contains("No caller account lookup, eth_call or callee fetch"));
        assert!(text.contains("https://sourcify.dev"));
        assert!(text.contains("\\u202e"));
        assert!(text.contains("\\u200b"));
        assert!(!text.contains('\u{202e}'));
        assert!(!text.contains('\u{200b}'));
        assert!(request.call.is_none() && request.caller.is_none());
        Ok(())
    }
}
