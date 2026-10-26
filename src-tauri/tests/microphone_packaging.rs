//! Contract checks for macOS microphone bundle metadata.

#[test]
fn macos_bundle_declares_microphone_purpose_and_sandbox_access() {
    let info = include_str!("../Info.plist");
    let entitlements = include_str!("../Entitlements.plist");

    assert!(info.contains("<key>NSMicrophoneUsageDescription</key>"));
    assert!(info.contains("only when you start local speech dictation"));
    assert!(entitlements.contains("<key>com.apple.security.device.audio-input</key>"));
    assert!(entitlements.contains("<key>com.apple.security.app-sandbox</key>"));
}
