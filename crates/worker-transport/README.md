# Fixed-role worker transport

This lower leaf owns authenticated control and anonymous pipe ownership for
three build-selected execution roles. It has no application codecs, source
broker, wallet, transaction or signing authority. Callers can select only the
compiler, public EVM discovery or frozen Base execution role. Paths, service
names, hashes, peer requirements and native profiles come from the build-owned
loaded main-image manifest, never an IPC/model request or a sidecar file.

The host checks fixed broker/worker file SHA256 pins and installs the exact
broker CDHash peer requirement before resuming XPC. Each independently launched
service creates one fixed hermetic worker, checks its actual running identity
and resources, then seals itself deny-default before accepting any message.
Ready returns only the direct worker stdin/stdout pipe endpoints. Application
data never traverses named XPC messages. The original upper-owner worker pins
must also match this transport cohort. Packaged production has no implicit
direct-launch fallback.

Finish and Cancel carry only a fresh per-session nonce. A reaping receipt is
accepted only from the authenticated established session after actual wait on
its owned worker, followed by termination of that original service connection.
This also prevents a subsequent same-role launch from reaching the previous
one-shot service before it exits. Connection end alone proves no worker cleanup.
A restarted service cannot claim an earlier child. Conflicting
or malformed receipts are sticky faults. Connection closure alone never proves
termination. Startup, execution and finish share the original absolute deadline;
all cleanup retries share one additional two-second grace. Missing post-Ready
cleanup evidence must poison a signed-runtime owner. A mandatory worker lifetime
pipe provides an additional broker-death guardian, without substituting for the
actual reaping receipt.

`native.rs` and the private native C bridge are narrow unsafe boundaries: kernel
peer authentication, loaded immutable manifest lookup and transfer of validated
anonymous pipe descriptors into uniquely owned Rust files. No public raw
descriptor, PID, arbitrary code requirement or certificate constructor exists.
Native callbacks hold explicit references and mutex-protected control state.
The role-specific native service owns child launch, resource enforcement and
its mandatory self-sealing profile. Its source and licence accompany staging.

The client dictionary/descriptor mutation test is deliberately a separate native
test program under `native/tests`. It does not launch services or mint production
capabilities. Rust unit tests cover the closed manifest grammar and file pins.
Neither replaces actual packaged App Sandbox service qualification or the human
security release gate.
