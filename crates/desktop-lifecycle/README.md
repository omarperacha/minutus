# Desktop lifecycle bridge

This leaf crate only registers non-secret macOS notifications. The normal desktop
wallet composition and isolated prototype use it; it has no wallet dependency.

The private `registration` module is the reviewed workspace unsafe-code exception
for Objective-C notification registration. Its safety invariants are:

- Registration and retained observer storage belong to the main thread.
- Each retained token is removed from the same retained centre before release.
- Foundation copies each block; the callback owns its `Arc` and captures no
  notification or native UI reference.
- Delivery uses no operation queue and can occur on any posting thread. The host
  callback is `Send + Sync`, receives only a lifecycle reason, revokes host
  readiness atomically, coalesces blocking cleanup and schedules AppKit work
  separately. The password-wallet host ignores ordinary app deactivation to
  match its existing wallet/x402 session policy. Signing re-observes foreground
  after authentication; system security events always cancel pending work.
- No notification payload is read, no caller-supplied object type is assumed,
  and panics cannot unwind through Objective-C.

Local notification tests cover retention and unregistration. Native lock/sleep
and rebuild/reseal acceptance passed on the recorded Apple Silicon Mac. Repeat
native lifecycle acceptance after relevant platform or notification changes;
synthetic observer tests cannot establish delivery of real OS events.

Brief screen lock does not necessarily sleep the display or resign the user
session. The bridge therefore also observes the distributed names
`com.apple.screenIsLocked` and `com.apple.screenIsUnlocked`. Both emit only a
security-boundary revocation. No unlock message grants readiness, so spoofing
can cause denial of service but cannot authorise a wallet operation. The concrete
distributed centre stays retained via a safe superclass upcast and owns its own
observer tokens; no payload is inspected and no new unsafe registration path is
introduced. Tests post those names only to a private in-process centre.

These OS names are an explicit compatibility assumption, not a documented secure
state API. [Apple documents distributed notifications](https://developer.apple.com/documentation/Foundation/DistributedNotificationCenter?language=objc)
as unauthenticated and asynchronous, with possible delay/drop, and delivered via
common run-loop modes. [Apple's run-loop guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/RunLoopManagement/RunLoopManagement.html)
includes modal panels in Cocoa's common modes. This matters while native approval
is open. Retain the workspace sleep/session signals and watchdog; real rapid
lock/unlock qualification remains mandatory after relevant platform/lifecycle
changes. The recorded native screen-lock acceptance passed after correcting a
frontend refresh delay; this does not establish reliable OS delivery on every Mac.
The names are also used by the
[Hammerspoon screen-lock watcher](https://github.com/Hammerspoon/hammerspoon/blob/master/extensions/caffeinate/libcaffeinate_watcher.m).
