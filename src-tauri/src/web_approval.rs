//! Native review of the complete outbound request, separate from redacted diagnostics.

use ipc_types::IpcError;
use web_retrieval::WebRequest;

/// JSON escapes make invisible formatting visible without changing wire bytes.
pub(crate) fn review_json(value: &serde_json::Value) -> String {
    use std::fmt::Write as _;
    let mut rendered = String::new();
    for ch in value.to_string().chars() {
        if ch.is_control()
            || matches!(ch,
            '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{180e}' |
            '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' |
            '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' |
            '\u{feff}' | '\u{e0001}' | '\u{e0020}'..='\u{e007f}' |
            '\u{e0100}'..='\u{e01ef}')
        {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                let _ = write!(rendered, "\\u{unit:04x}");
            }
        } else {
            rendered.push(ch);
        }
    }
    rendered
}

fn request_text(request: &WebRequest) -> Result<String, IpcError> {
    // Keep the canonical wire spelling, including percent escapes. Decoding it
    // could turn attacker-controlled escapes into misleading line breaks.
    let mut text = format!(
        "Method: {}\nOrigin: {}\n\nComplete URL sent to the website:\n{}",
        request.method().as_str(),
        request.url().origin(),
        request.url().as_str()
    );
    if !request.body().is_empty() {
        text.push_str("\n\nJSON body (formatting controls escaped for review):\n");
        let value = serde_json::from_slice(request.body()).map_err(|_| IpcError::internal())?;
        text.push_str(&review_json(&value));
    }
    Ok(text)
}

#[cfg(target_os = "macos")]
pub(crate) fn review(app: &tauri::AppHandle, request: &WebRequest) -> Result<bool, IpcError> {
    use objc2::{MainThreadMarker, rc::autoreleasepool};
    use objc2_app_kit::{
        NSAlert, NSAlertSecondButtonReturn, NSAutoresizingMaskOptions, NSScrollView, NSTextView,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::sync::mpsc;

    let text = request_text(request)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let decision = autoreleasepool(|_| {
            let mtm = MainThreadMarker::new().ok_or_else(IpcError::internal)?;
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&NSString::from_str("Allow web request?"));
            alert.setInformativeText(&NSString::from_str(
                "Review the complete URL below. Its URL and any displayed request body will leave your Mac and may contain private data. Remote content is untrusted. Redirects are disabled; payment requires separate approval.",
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
pub(crate) fn review(_app: &tauri::AppHandle, _request: &WebRequest) -> Result<bool, IpcError> {
    Err(IpcError::permission_denied(
        "Complete native web review is unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_retrieval::{CanonicalWebUrl, WebMethod};

    #[test]
    fn native_review_distinguishes_payloads_hidden_by_diagnostic_redaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let benign = WebRequest::new(
            WebMethod::Get,
            CanonicalWebUrl::parse("https://example.com/report?topic=weather")?,
        );
        let leaking = WebRequest::new(
            WebMethod::Get,
            CanonicalWebUrl::parse("https://example.com/private-data?leak=synthetic-secret")?,
        );
        assert_eq!(
            benign.permission_descriptor().resources(),
            leaking.permission_descriptor().resources()
        );
        assert_ne!(request_text(&benign).ok(), request_text(&leaking).ok());
        assert!(
            request_text(&leaking)
                .map_err(|_| "review failed")?
                .contains(leaking.url().as_str())
        );
        Ok(())
    }

    #[test]
    fn native_review_preserves_long_urls_and_encoded_controls()
    -> Result<(), Box<dyn std::error::Error>> {
        let url = format!("https://example.com/%0A?value={}", "x".repeat(3_900));
        let request = WebRequest::new(WebMethod::Head, CanonicalWebUrl::parse(&url)?);
        let text = request_text(&request).map_err(|_| "review failed")?;
        assert!(text.ends_with(request.url().as_str()));
        assert!(text.contains("/%0A?value="));
        assert!(text.starts_with("Method: HEAD\n"));
        Ok(())
    }
}

#[cfg(test)]
mod x402_post_tests {
    #[test]
    fn review_includes_exact_outbound_json_body_and_method()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = web_retrieval::WebRequest::json_post(
            web_retrieval::CanonicalWebUrl::parse("https://example.com/order")?,
            br#"{"item":"sample","quantity":2}"#.to_vec(),
        )?;
        let text = super::request_text(&request).map_err(|_| "review failed")?;
        assert!(text.contains("Method: POST"));
        assert!(text.contains("https://example.com/order"));
        assert!(text.contains(r#"{"item":"sample","quantity":2}"#));
        Ok(())
    }
}

#[cfg(test)]
mod formatting_tests {
    #[test]
    fn native_json_review_exposes_controls_and_preserves_wire_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = serde_json::json!({
            "item": "report\u{202e}amount\u{2066}\u{200b}\u{e0020}",
            "literal": "\\u202e", "language": "日本語 café", "lines": "one\ntwo"
        });
        let bytes = serde_json::to_vec(&input)?;
        let request = web_retrieval::WebRequest::json_post(
            web_retrieval::CanonicalWebUrl::parse("https://example.com/order")?,
            bytes.clone(),
        )?;
        let display = super::review_json(&input);
        assert!(!display.contains('\u{202e}'));
        assert!(!display.contains('\u{2066}'));
        assert!(!display.contains('\u{200b}'));
        assert!(!display.contains('\u{e0020}'));
        assert!(display.contains(r"\u202e"));
        assert!(display.contains(r"\\u202e"));
        assert!(display.contains("日本語 café"));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&display)?, input);
        assert_eq!(request.body(), bytes);
        assert!(
            super::request_text(&request)
                .map_err(|_| "review failed")?
                .contains(&display)
        );
        Ok(())
    }
}
