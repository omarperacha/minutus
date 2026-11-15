# Bounded local execution worker

This leaf owns the fixed child protocol, pinned upstream engine adapters and
one-shot macOS process lifecycle. `chain-evm` owns RPC and proof-checked state;
the desktop owns any future consent, packaging and write admission. No network,
wallet, signing, app storage, project grant or model orchestration dependency is
allowed in this crate. Dependency policy enforces that direction.

`contract.rs` contains authority-free values and fixed limits; `admission.rs`
selects explicit reviewed chain/fork windows. `protocol.rs` owns bounded framing
and state sequencing. `engine/` adapts maintained Revm/Base implementations,
never a home-grown interpreter. `supervisor/` owns the qualified remote lifecycle,
unqualified local diagnostic lifecycle, bounded pipe exchange and actual-child
reaping. `worker.rs` composes one request after applying limits and a mandatory
FD3 owner-loss guardian.

The private `native_resources.rs` subtree has a narrow unsafe-code exception:
it closes inherited descriptors except stdio and the checked read-only FD3 owner
pipe, validates that pipe, and queries SDK resource counters for an owned local
diagnostic child. Its guardian exits immediately on EOF, any byte or read error;
it carries no commands, source, signature or output. All other inherited
non-stdio descriptors are closed before guest execution. The task-name fallback
is read-only, checked for complete SDK output and released once; it grants no
task control or address-space access. Resource accounting failure is terminal.
The separate private `native_identity.rs` exception copies only the immutable
worker pins from the loaded main image. No arbitrary PID, descriptor or raw
trusted-identity constructor is exposed.

Qualified packaged execution uses `worker-transport` exclusively. Fixed-role
XPC brokers launch, attest and account the actual deny-default sandboxed worker,
then confine themselves before Ready. The host cross-checks original worker
SHA256/CDHash pins against that transport cohort before starting. Source/state
and signed-envelope protocol bytes use only authenticated anonymous pipe FDs;
XPC carries nonsensitive lifecycle control and actual-worker reap receipts.
There is no direct fallback on missing services or transport failure.

The worker may read only its fixed image and system loader libraries. Network,
user files, process creation and arbitrary executables remain denied. Only
protocol stdio and the owner-loss pipe survive startup. Broker cancellation and
resource failure kill and reap the actual child. Guest EOF or connection loss
cannot manufacture a reap receipt. An unconfirmed offline termination places
this host process in terminal quarantine: new packaged workers, simulation
controls and chain acquisition must reject without a reset API. Nonblocking
host pipe IO can stop even if remote termination is unconfirmed. The FD3
guardian contains broker death but is never treated as proof of reaping.

Raw-path developer diagnostics remain unqualified, use mandatory local
sandbox-exec with the same limits/guardian, and cannot mint a trusted execution
certificate. They may be unavailable inside an App Sandbox host. Other platforms
have no qualified launch path and fail closed. Online child observations retain
`effects_qualified=false`; Base signed results remain opaque until the host
checks exact material against its independently produced certificate. See the
[execution policy and qualification limits](../../docs/local-evm-simulation.md)
and [dependency review](../../docs/local-simulation-dependency-review.md).

`replay/` is the separately reviewed host execution TCB. It runs the maintained
engine over frozen values and privately constructs an execution certificate;
proof provenance and material semantics remain with chain-evm. Its pre-signature
discovery callback only acquires missing proof facts; final frozen replay has no
callback. Detailed traces preserve restored/reverted SSTORE attempts, KECCAK256
preimages and real native transfers separately from apparent delegate value.

Host replay has fixed gas/steps/memory and cooperative cancellation. It is not a
killable process. Standard native precompiles and Base's maintained static and
dynamic precompile lookup are rejected before application dispatch. Base
application-frame mode is explicitly partial: zero-value fee-independent material
execution with conservative prepayment, followed by a mandatory genuine-envelope
native offline comparison. It never fabricates an envelope or projects signed
child data. See the execution policy for the proof and admission exclusions.
