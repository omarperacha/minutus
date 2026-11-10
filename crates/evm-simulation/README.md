# Bounded local execution worker

This leaf owns the fixed child protocol, pinned upstream engine adapters and
one-shot macOS process lifecycle. `chain-evm` owns RPC and proof-checked state;
the desktop owns any future consent, packaging and write admission. No network,
wallet, signing, app storage, project grant or model orchestration dependency is
allowed in this crate. Dependency policy enforces that direction.

`contract.rs` contains authority-free values and fixed limits; `admission.rs`
selects explicit reviewed chain/fork windows. `protocol.rs` owns bounded framing
and state sequencing. `engine/` adapts maintained Revm/Base implementations,
never a home-grown interpreter. `supervisor.rs` owns launch, independent resource
monitoring, pipe exchange and unconditional child reap. `worker.rs` composes a
single request after applying resource limits.

The only unsafe-code exception is private `native_resources.rs`: it closes all
inherited non-stdio descriptors in the single-threaded child before execution
and queries resource usage for the exact unreaped child owned by the supervisor.
Buffers use the SDK's exact structures, checked sizes and successful kernel
initialisation. No arbitrary PID/descriptor or memory-reading API is exposed.
Darwin descriptor-list sizing can overallocate; only the checked returned prefix
is inspected. A failure to close/account/isolate is terminal. This isolated FFI is
safer than pretending macOS address-space limits reliably constrain allocation.

macOS sandbox-exec is mandatory. Only the fixed worker image and system loader
libraries are readable, with the literal root directory open required by Darwin
(no child-file permission). Environment is empty; only protocol stdio survives.
Network, user files, process creation and arbitrary executables remain denied.
The parent monitors physical footprint independently of state IO and kills/reaps
on cancellation, deadline, protocol or resource failure. Trusted state adapters
must independently enforce the supplied IO deadline/cancellation contract.
Other platforms have no qualified launch path and fail closed.

This is a developer API, not yet a packaged chat simulation or signing route.
Every observation has `effects_qualified=false`. The native Base handler's full
envelope requirement remains unpassed. See [execution policy and qualification
limits](../../docs/local-evm-simulation.md) and [dependency review](../../docs/local-simulation-dependency-review.md).


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
