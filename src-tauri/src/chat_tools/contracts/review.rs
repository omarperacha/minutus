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
                "Review the provider, target, caller and exact calldata below. These bytes will leave your Mac and may contain private data. This is one-time outbound permission, not transaction approval. No wallet unlock or signing.",
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
    format!(
        "Allow this one-time outbound request to the configured RPC?\n\nNetwork: {}\nProvider origin: {}\nTarget: {}\nBlock: {}\n{}\n\nThe provider sees your IP address and these public addresses/call bytes. Arguments may contain sensitive data: deny if you do not intend to share them. This permission covers the displayed call and its chain/block/code checks only.\n\nNo wallet, password, signature or transaction. Simulated state changes are discarded. ABI and provider data are unverified; code presence and call success do not establish safety or effects.",
        request.network.descriptor().display_name(),
        endpoint.public_origin(),
        request.target,
        block,
        call
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
            network: chain_evm::EvmNetwork::BaseSepolia,
            target: call.target().into(),
            block: ContractBlock::Latest,
            caller: None,
            call: Some(call),
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
            network: chain_evm::EvmNetwork::EthereumMainnet,
            target: call.target().into(),
            block: ContractBlock::Latest,
            caller: None,
            call: Some(call),
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
}
