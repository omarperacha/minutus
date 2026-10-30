# Agent and tool design

Status: active

This reference governs the in-app agent controller, model-facing tool contracts,
context handling and recovery. These are Minutus product requirements, not
instructions for how a contributor's coding assistant should execute a task.
For general coding conventions, read [engineering house style](engineering-house-style.md).
Read this reference when changing agent behaviour, tools or orchestration; the
[crate boundaries](../crates/README.md) remain authoritative.

- [Tool contracts](#tool-contracts)
- [Planning and admission](#planning-and-admission)
- [Context and prompt caching](#context-and-prompt-caching)
- [Discovery and request binding](#discovery-and-request-binding)
- [Semantic execution and adaptation](#semantic-execution-and-adaptation)
- [Evidence tools and diagnostics](#evidence-tools-and-diagnostics)
- [Checkpoints and host phases](#checkpoints-and-host-phases)
- [Reasoning budgets and recovery](#reasoning-budgets-and-recovery)
- [Evidence reuse and batch repair](#evidence-reuse-and-batch-repair)
- [Completion and verified outcomes](#completion-and-verified-outcomes)

## Tool contracts

- Give each tool one discoverable responsibility. Distinguish path discovery,
  content search, aggregate analysis, semantic inspection and mutation rather
  than adding unrelated modes to one broad tool.
- Prefer bounded batched inputs when several independent queries share the same
  operation and trust boundary. Return one structured result per query in request
  order, including exact coverage, omission and truncation metadata.
- When a later operation consumes a path returned by discovery, provide a bounded
  conversation-scoped host reference beside the exact human-readable path. Restore
  bounded mappings from paired successful discovery receipts across follow-ups and
  application restarts. Bind new identities to the conversation, stable workspace
  grant and exact path; permission changes do not change identity. Let exact-path
  consumers (including exact evidence items) resolve references through their
  existing path arguments, then apply current permissions and ordinary broker
  validation. Reject unknown, ambiguous or cross-workspace references before
  broker access; never reassign an old identity to another path. Keep literal paths available
  for user-provided and already-known resources; never use fuzzy path correction.
- Name model-facing operations for their observable effect and put them in the
  namespace that owns that effect. Prefer `filesystem.write_delimited_table` to
  an orchestration verb such as `transform_artifact` when the result is exactly
  one verified CSV or TSV write. Use action classes such as `compute` and
  `workspace_write` that mean the same thing as their compatible operations.
- Give every model-facing operation a least-surprise contract: say when to use
  it, when not to use it, define every argument and meaningful result, identify
  important failure behaviour and include one valid generic example for nested or
  format-sensitive input. Examples use neutral placeholder paths and content;
  never teach an evaluation fixture or product-specific hidden convention.
- Prefer separate effect-oriented tools and flat closed arguments over mode flags,
  nested representation unions or names that require the model to infer an
  implementation abstraction. Consolidate operations only when their effect,
  authority, validation and result contract are genuinely the same.
- Advertise `additionalProperties: false`, explicit required fields, bounded
  arrays and closed enums wherever practical. Enable provider-enforced strict
  calls when an adapter supports them, but always validate the same host-owned
  schema before permission checks or execution. Return the exact failing JSON
  path, expected type or enum and a valid generic example for repairable model
  errors. When one structured artefact call can contain many independently
  detectable defects, aggregate them with exact paths and expected/received
  cardinalities so one continuation can repair the whole call. Keep the accepted semantic
  step and checkpoint intact, and re-expose the rejected operation plus bounded
  same-step alternatives for changed-input repair.
- Version incompatible tool and action vocabulary. Keep old parsers and operation
  handlers only for persisted history that declares the old version; do not
  advertise competing aliases to new runs. Normalise only transport-equivalent
  syntax that cannot alter semantic intent.
- A model-facing orchestration tool may compose narrow internal primitives when
  its single responsibility is binding dependent evidence into one bounded host
  operation. Keep parsing, discovery, loading and mutation boundaries separate;
  reuse their existing validation, capability checks, auditing and limits.
- Put stable outcome-oriented judgment in the system policy once. Put changing
  counters, warnings and request state in host-owned dynamic context rather than
  accumulating task recipes in permanent prompt prose.

## Planning and admission

- Keep five orchestration contracts separate: user intent and authorisation,
  host permission policy, semantic execution plan, adaptive operational route
  and authoritative operation ledger. User outcome, scope, constraints, approval
  boundaries and success criteria are binding. The plan cannot mint a capability,
  approval, trusted result or verified postcondition.
- Classify operation admission on three independent axes: permission outcome,
  relationship to the current semantic step and observed progress contribution.
  A plan match never substitutes for capability, workspace, effect or approval
  validation; a plan mismatch is not itself a permission denial. Keep these
  classifications typed and content-free in development telemetry.
- Require a concise, versioned semantic execution plan before workspace content,
  computation, mutation or other broader tool-backed work. Granted path metadata
  (`filesystem.list` and `filesystem.find_paths`) and metadata-only Bazaar
  discovery may precede planning; normal argument, scope and broker validation
  still applies. Mixed discovery/effect batches remain deferred. Sufficient
  discovery can support a direct answer or missing-input question without a
  bookkeeping plan. A bounded public-web answer also needs no plan: expose
  `web.search`, `web.retrieve` and
  `orchestration.submit_plan` as one stable initial surface. Let the model's first
  call select direct retrieval or the semantic-plan escape without a separate
  intent-classification inference. Direct web retrieval uses the ordinary
  call-result-continuation loop, has no semantic stages or `complete_stage`
  calls, and answers as soon as sufficient cited evidence is available.
  A standalone `wallet.request_transfer` is also directly callable as the sole
  call in its batch, without semantic stages. It retains exact native review,
  fresh authentication and durable one-attempt enforcement. Mixed batches and
  broader work retain planning admission.
  Permission mediation, source-safety inspection, broker limits and host
  validation remain unchanged. If broader work becomes necessary, accept the
  plan escape and retain successful direct-web evidence in selected history.
  For `semantic_execution_v2` through `semantic_execution_v4`, accept that plan by
  itself, then require one separately validated advisory execution forecast in the
  next round. V5 retains its historical host-routed obligation behaviour only for
  persisted history. Advertise `semantic_execution_v6`: retain the clean plan-only
  validation round, then run the ordinary call-result-continuation loop without a
  model-authored forecast or host-enforced operation frontier. Preserve v1-v5
  parsing without reinterpretation. Text-only answers need no plan or forecast.
- Keep legacy execution forecasts advisory and versioned. Validate exact semantic-step
  coverage, dependencies, evidence/effect coverage, closed categories, suggested
  operations and known-versus-unresolved argument names, but never treat a route,
  order or resolvedness claim as permission, progress or a verified postcondition.
  Forecast only tool-backed work; orchestration controls, reasoning, reporting and
  final responses cannot forecast themselves. Emit exact nested repair paths and
  keep forecast telemetry content-free.
- Declare tool category membership once in catalogue metadata. Project a compact
  category index containing stable names, plain-language responsibilities and
  available operation names into every tool-orchestration round. Categories are
  documentation and context-selection metadata only: they cannot authorise an
  effect, route execution, prove progress or override exact operation validation.
  Keep the existing `filesystem.*`, `computation.*`, `web.*` and
  `orchestration.*` operation names stable; do not create deeper namespaces merely
  to restate category membership.

## Context and prompt caching

- Keep active tool documentation bounded without weakening contracts. Planning
  receives the plan schema, forecast validation receives the forecast schema, and
  v2-v4 execution receives a small common control set plus complete schemas for at
  most two dependency-ready forecast categories. V5 execution receives complete
  schemas for operations whose host catalogue action class matches a dependency-
  ready semantic step, with typed obligation and permission gates applied before
  exposure. These are legacy execution policies. V6 omits obsolete forecast
  controls and directly exposes complete schemas for every plan-wide suggested
  task tool. Keep those definitions in canonical order for the accepted plan.
  Also expose `orchestration.search_tools`: it searches the complete host
  catalogue and loads matching complete task schemas on the next round without
  revising the plan, changing authority or terminating the current stage. This
  discovery escape keeps every available task capability reachable without
  paying the prompt cost of every schema on every ordinary round. A material plan
  revision or an explicit search result is a legitimate tool-surface boundary;
  otherwise the active definitions must remain byte-stable. Promote the current
  stage's suggestions in the appended execution-state capsule. Keep `orchestration.complete_stage`
  exposed as a stable no-argument advisory semantic cursor control; the host
  already owns the single current stage and validates only that the boundary is
  structurally unambiguous and has a current dependency-ready stage. Do not ask the model for a
  stage ID or redundant completion summary. Within a projected legacy category expose a
  bounded set of credible same-step alternatives. Never truncate a selected
  operation's descriptions, strict schema or generic examples. Forecast revision
  expands another category when evidence changes the route, and the bounded safe
  read recovery path may re-expose one omitted non-effecting operation. An omitted
  schema is never a permission denial or a permanent capability decision.
- Keep the cache-critical prompt prefix stable within an execution surface. Put
  immutable policy and canonical tool definitions first. Append changing budgets,
  ledger state, recovery diagnostics, the current stage, completed-result IDs and
  promoted suggestions in one compact, non-persisted host capsule at the end of
  selected history. Native Qwen may retain earlier admitted capsules in the
  request-local provider transcript when exact base history, model binding and
  image identity extend unchanged. The newest capsule supersedes them; they
  never enter saved dialogue, recall, summaries or authority. Count their bytes
  in exact admission and discard replay before ordinary pressure compaction.
  Do not rewrite the leading system message, reorder tools or
  specialise tool schemas merely to steer the next action. Plan admission,
  material plan revision, explicit tool-search loading and first authorisation
  of a bounded safe-read escape may establish a new stable surface; ordinary
  stage progression and repeated use of an already-visible safe read may not. A
  transition to `final_response_ready` after explicit completion of every advisory stage and
  any required verified effect closes the execution surface and intentionally exposes
  no tools for the final assistant response; this is not a schema-prefix
  mutation. A catalogue-declared one-shot operation may treat its first
  authoritative non-partial result as its advisory completion boundary when
  success, denial and clear failure all exhaust the user-authorised attempt.
  This exception must proceed to reporting and must not generalise failed work
  into progress for ordinary or retriable operations. Consumed operations must
  also be absent from tool-search results; search cannot promise a schema that
  the request-local ledger has retired. If the model nevertheless emits one safely bounded historical tool
  call in this state, return one non-terminal final-response correction that says
  the verified outcome is complete, the tool surface is intentionally closed and
  a plain-text response is required now. Do not reuse generic inactive-tool
  guidance, enumerate an empty tool list, or suggest search, forecast or plan
  revision after completion. Regression coverage must exercise the complete positioned boundary:
  `orchestration.complete_stage`, a dependency-ready verified write after it in
  the same batch, and the subsequent model decision to finish or continue.
  A write-derived cursor advance alone must keep tools available: one stage may
  cover several deliverables. A normal final answer needs no extra completion
  calls for unacknowledged advisory stages; preserve their actual status.
  Move rich-input reminders, summaries and mutable ledger projections
  to the tail as well. Measure admitted
  tokens, active tool count and contract bytes, provider prompt/cache metrics,
  prompt-evaluation duration, model rounds, calls, correctness and end-to-end
  latency on representative runs; fewer calls alone are not evidence of
  improvement. Distinguish an exact host-measured reusable prefix from a
  provider-reported cache hit. A provider prompt-token count may describe the
  complete admitted prompt even when KV state was reused; do not rename it as an
  evaluated-token or cache-miss count without an explicit provider guarantee. If
  the provider exposes only prompt count and evaluation duration, keep cache-read
  and cache-write counters absent rather than deriving them from the shared
  prefix or timing.
- For bounded direct-web answers, do not append a rewritten per-round
  orchestration capsule when the stable policy and fixed tool contracts already
  express the complete route. Keep reasoning strength and provider options
  stable, append paired calls and results monotonically, and retain the same
  canonical tool order until a semantic plan is submitted. A plan escape is an
  intentional new prefix boundary; ordinary search, retrieval and final-answer
  continuation are not.
- Keep render-only admission and the corresponding inference request identical
  for every provider runner-affecting option, including context size, projector,
  adapters, parallelism and context-shift policy. Construct both through one
  typed boundary. A provider may schedule or reload a model before returning a
  rendered template, so an apparently read-only preflight with different runner
  options can destroy resident KV state. Production-path cache probes must enter
  through that same configured boundary; a lower-level probe is insufficient.
- Keep completed work for every dependency-ready stage in focused context, not
  only the single advisory cursor and its ancestors. This is especially important
  when a positioned completion call is followed by ready independent image work:
  the returned image must be available in the immediately following round even if
  another stage remains current. Present identical normalised rich-input bytes at
  most once per prompt by verified content digest, not opaque asset ID. Retain
  the bounded current-request source set across advisory stages, tool search,
  revision and write repair. Keep the earliest selected occurrence in its stable
  history position, including when explicit reinspection returns an identical
  alias. Delivery is not a substitute for evidence availability. A new
  asset ID alone does not justify a duplicate image. Track retrieval separately from
  provider presentation: retain pending images across preparation/connection
  failure, and charge the inspection allowance only when a model response
  confirms delivery. Pending images must retain their paired tool history even
  outside ordinary stage focus. Recovery preserves original pixels and established
  results, not the exhausted round's uncommitted reasoning. Presentation remains
  a transport fact, never evidence of comprehension or stage completion.

## Discovery and request binding

- Interpret relevant-tool exposure as a bounded credible option set, not a
  single predicted happy path. Keep recovery controls such as semantic-plan
  revision available whenever they may be needed to describe materially changed
  work; do not hide them merely because the current plan is executable. Reduce
  ambiguity through precise contracts and host validation rather than by removing
  plausible recovery choices. An omitted v6 task schema remains searchable and
  loadable through `orchestration.search_tools`; omission is never a permission
  denial or evidence that the capability is unavailable. Project the revision schema conditionally: its
  `mode` enum may require `revise` if selected, but its description must call it
  an optional recovery control and state the material trigger. Never describe an
  available revision mode as work the current host state "requires".
- When a host-owned execution value is drawn from current request state, project
  that value into the active JSON Schema. In particular, constrain collector
  `items[].id` with the exact dependency-ready task-local obligation IDs instead
  of advertising a generic string and explaining the hidden equality rule only in
  prose. This task-local obligation projection applies to the legacy v5 contract,
  not v6. Retain host-side semantic validation and return the received value,
  accepted task-local IDs and exact repair path through the normal continuation
  loop if schema enforcement is unavailable or fails.

## Semantic execution and adaptation

- Plan only tool-backed task stages. In advertised `semantic_execution_v6`, each
  stage states its objective, dependencies, observable completion condition,
  broad action class and one or more `suggestedTools`. Suggestions must name
  advertised operations that serve the objective, are promoted plan-wide after
  acceptance, and are never an allow-list, permission rule or progress
  receipt. Do not require argument forecasts, evidence obligations, receipt IDs
  or a separate execution forecast. Keep the plan concise enough that it helps
  the model act rather than becoming a second programming language.
  Keep plural plan fields structurally unmistakable in both the advertised schema
  and planning capsule: `assumptions`, `constraints`, `successCriteria`, `steps`,
  `dependsOn` and `suggestedTools` are native JSON arrays, never quoted or
  JSON-stringified arrays. V6 permits omitted `dependsOn`, meaning no declared
  dependencies; supplied dependencies retain type, reference and cycle validation.
  Do not coerce malformed strings host-side. Aggregate
  independently detectable v6 defects and return exact JSON paths, received types
  and permitted replacements so one continuation can repair the plan without
  rewriting unrelated valid content.
  Do not equate a stage's intended work with a multi-operation tool's catalogue
  class. V6 validates availability, not that equality: enforcing it caused real
  discovery requests to oscillate between unrelated suggested tools. Actual
  operation arguments, permissions and effect verification retain their owners.
  Apply the same transport clarity to structured artefact operations.
  `filesystem.write_delimited_table` takes its six fields directly at the top
  level; `columns` and `rows` are native JSON arrays, never a `changes` or
  `representation` wrapper or a JSON-encoded string. Report missing and
  unexpected top-level fields, invalid cell paths and rows whose cell count
  differs from the header together in one bounded repair result; if the explicit
  diagnostic cap is exceeded, report the omitted count rather than emitting an
  unbounded error.
  Keep image inspection and artefact creation as separate operations, but make
  their handoff explicit in both complete definitions and truncated discovery
  summaries. `filesystem.inspect_image` reads one image and points table-shaped
  outcomes to `filesystem.write_delimited_table`; the writer is the canonical
  structured consumer for image-to-CSV, image-to-table and screenshot-to-table
  work and takes extracted values directly in its top-level `columns` and `rows`
  arrays. Point other text-file outcomes to `filesystem.write_text`. Maintain a
  small closed set of outcome-oriented search aliases in catalogue metadata so
  equivalent hyphenated or spaced intent queries find the reader and preferred
  consumer. Use alias ranking only for discovery order: it grants no authority,
  hides no credible alternative and creates no cross-tool state or universal
  image-to-artefact operation.
  Promote the canonical table writer beside an image reader or collector in the
  stable v6 plan-wide tool surface when that writer is present in the available
  catalogue. Keep generic writers and search available; companions never grant
  workspace permissions or force an immediate consumer call.
  In legacy `semantic_execution_v4` and `semantic_execution_v5`,
  declare obligations with a bounded task-local ID and one closed, observable
  `kind`. Evidence kinds also carry a closed `roleKind` only where the role changes
  task meaning; outcome kinds such as `computation_completed` and
  `file_write_verified` do not expose host evidence classes, postconditions or
  effect roles. Derive those internal fields host-side. Make each action branch
  admit only compatible kinds so invalid cross-field combinations are absent from
  the advertised schema. Begin the closed evidence-role vocabulary with `source`,
  `convention`, `target`, `input` and `reference`; extend either vocabulary only
  in a versioned contract. Preserve `semantic_execution_v3` as the explicitly
  parseable typed-tuple contract and v2 as the roleless `requiredEvidence`
  contract, without reinterpretation.
  Keep operation choices out of v1-v5 semantic plans. In v2-v4 place them in the
  separate advisory forecast; in v5 derive the available operations from trusted
  catalogue metadata, the ready dependency frontier and host-owned obligation
  gates. V6 deliberately includes only non-binding suggested tool names so the
  model can discover the likely route without surrendering runtime flexibility.
  When several dependency-ready stages suggest the same operation, promote that
  operation once in stable first-seen order. Every advertised prompt contains at
  most one definition for a tool name; parallel-stage projection must never
  duplicate schemas at the model-adapter boundary.
  Retain optional, non-exhaustive
  `preferredTools` only while parsing legacy contracts; omission never denies a
  safe operation and inclusion never authorises one. Do not represent reasoning,
  composing, formatting, reporting or the ordinary final assistant response as
  plan steps.
- For legacy v3-v5 execution, credit a typed obligation only from an executed
  operation and host-verified result capability. Bind receipts to immutable
  call/result provenance, never to
  model descriptions, filenames or plan claims. Do not reuse one result output
  for two obligations. A bounded collector may satisfy several obligations only
  when each obligation ID names a distinct structured result item and every item
  independently proves its declared media/postcondition capability. Keep
  outcome-bearing operations out of the active projection until all prerequisite
  source and convention obligations are separately grounded; expose the exact
  missing obligation as a recoverable plan mismatch.
- Validate semantic dependency graphs as graphs: require unique identifiers,
  known dependencies and acyclicity, then derive a stable topological execution
  order. Do not make array position a second dependency language, and do not add
  a parallel `requires`/`produces` graph unless the existing dependency model is
  shown by representative failures to remain ambiguous after obligation
  simplification.
- Aggregate every independently detectable plan-admission defect into one
  structured correction. Identify each defect by contract version, step, path and
  diagnostic code; return its offending projection and exact permitted object.
  Preserve unrelated plan content during repair. Count repair progress against
  the defect set: removed defects or changed offending projections progress,
  unrelated edits do not, and newly introduced defects never reset the bounded
  allowance. When fixing a coarse structural defect exposes a deeper defect,
  the removal of the previously reported defect still earns the next bounded
  repair; merely adding defects while every prior defect remains unchanged does
  not. Keep validation host-side and return recoverable failures through the
  normal model-visible result loop.
- Keep v6 same-stage adaptation flexible and explicit. Argument repair, an
  alternative available tool, discovery, bounded refinement and transient retry
  do not require revision and must not be rejected solely because they were not
  suggested. A failed or incomplete operation leaves the current stage active and
  the catalogue usable. A changed deliverable, target, dependency graph, success
  criterion, user preference or action-risk class may use the optional bounded
  revision escape. For legacy contracts, argument repair, an equivalent
  tool in the same action class, path discovery, evidence refinement needed by
  the current objective and transient retry do not rewrite the plan. A changed
  deliverable, target, dependency graph, success criterion, user preference or
  action-risk class requires a versioned revision.
- Treat route suggestions as hints in implementation, not only in documentation.
  V6 progress may come from an omitted compatible tool; supporting calls outside
  the current broad action class remain legal and leave stage focus unchanged.
  Successful task calls add their results to the current dependency context but
  do not imply that a read or compute stage's natural-language `doneWhen`
  condition is true. Advance those stages only when the model calls the explicit
  `orchestration.complete_stage` control for the single host-owned current stage.
  Treat the control as the model's semantic judgment: it may be called alone and
  must not require a host-inferred receipt count, retained prose extraction or an
  immediate successor consumer. Keep its advertised schema stable and closed with
  no arguments; supply the current stage ID, objective, `doneWhen`, suggestions
  and established result IDs in the host-authored execution tail. A host-verified outcome-bearing write
  may advance its write stage directly. Reject stale or dependency-blocked stage
  completion attempts through the ordinary result-continuation loop. Incomplete
  discovery and failed calls leave the stage active. Do not emit plan-mismatch errors or consume
  correction allowances merely because an operation was not suggested.
  Classify prompt-preparation failures with closed, content-free diagnostic
  subtypes, including duplicate or otherwise invalid tool definitions, before
  returning the ordinary terminal category. Never include prompt text, arguments,
  paths, tool names or model content in execution telemetry.
  If an image-bearing v6 round exhausts its output allowance without acting,
  retain the image and established results but switch the recovery pass to low
  reasoning with enough bounded output for structured arguments. Keep the full
  catalogue available. Recovery guidance must follow the active stage: emit
  `orchestration.complete_stage` when `doneWhen` is already satisfied, otherwise
  one suitable task call; demand a write only when a dependency-ready write stage
  is actually current. Never ask the model to repeat transcription or arithmetic
  in private reasoning. A successful recovery call returns execution to ordinary
  stage selection.
  If a dependency-ready stage accumulates two successful task results without an
  explicit completion decision, promote the completion decision in the tail and
  use a low-reasoning bounded-action pass only without active visual work. Never
  downgrade an image-bearing extraction merely because a checkpoint is due.
  This reminder is non-terminal, does not imply completion and
  does not narrow the task-tool catalogue or remove optional revision.
  Keep successful result identities attached to the stable stage across plan
  wording changes. In v6, a stable read/compute stage ID with the same action class
  and dependency set also retains its advisory completion declaration. This is an
  explicit identity rule, not host inference that two prose descriptions mean the
  same thing. Use a new stage ID (and update dependents) to reopen or replace work.
  Changed dependencies or action class reopen completion; effect-stage completion
  retains exact work matching and changed outcome bindings clear verification.
  Legacy evidence equivalence remains exact. If a revision renames or replaces
  that stage, retain a bounded
  set of its still-live host results as revision working context without carrying
  forward the old semantic-completion judgment. A plan revision must not erase a
  discovered path or loaded input merely because model-authored objectives or
  completion wording changed.
  Every focused execution capsule must keep the overall target and success
  criteria alongside the current stage. Name the successful result call IDs whose
  selected tool-result history is present, and tell the model first to compare
  those established results with `doneWhen`. If satisfied it should complete the
  stage; otherwise it may use one suitable call to obtain a specific missing fact.
  This guidance must explicitly avoid asserting that any fact is missing and must
  remain non-terminal.
  Learned facts, completed results and changed advisory tool preferences are not
  material plan revisions. For v6, acknowledge assumption-only or route-hint-only
  resubmissions as an unchanged plan, preserve the current cursor and completion
  state, do not consume revision capacity, and return the current stage as the
  continuation. A material revision changes the target, constraints, success
  criteria, stage work or dependencies.
  Treat the position of the single `orchestration.complete_stage` control as an
  explicit in-order boundary. Task calls before it execute against the current
  stage; the declaration uses established context and any results returned before
  that model response; and task calls after it execute against the newly ready frontier.
  Project the exact post-boundary stage IDs, objectives and suggested tools in the
  execution tail. Reject multiple completion controls or mixed
  orchestration controls through ordinary non-terminal recovery. Never send an
  orchestration control to the generic workspace executor. When one response can
  both declare the current stage complete and form a dependency-ready operation
  from the same established results, permit the boundary followed by that task
  operation. Promote the newly ready operation's complete contract in that
  boundary-decision capsule so the model need not infer it from the full
  catalogue. In v6, one structurally valid, argument-free completion checkpoint
  per batch is charged to the existing round limit, not the task-call allowance.
  Admission and accounting use the same cost, so a checkpoint cannot displace an
  otherwise affordable write. Other calls, including plan/search controls and
  failed or deferred task attempts, remain charged. Multiple or malformed
  completion controls receive no exemption. Raw telemetry still counts every
  emitted call. Never partially execute an over-budget effect batch or relax its
  ordinary argument, permission and stage validation.
  Ordering guides stage attribution but must never erase an authorised,
  host-verified effect. If an outcome-bearing write appears immediately before a
  valid completion boundary, retain its exact result; after the boundary makes the
  write stage dependency-ready, reconcile that result only when its action class
  and verified postcondition match the stage. This grants no authority and does
  not infer completion of a read or compute stage. Until reconciliation, keep the
  actual current semantic stage and the verified result visible; never project
  `outcomeVerified=true` with a null current stage and mutation guidance. Once the
  explicit predecessors and verified outcome are complete, promote a final-answer
  decision instead of rediscovering or repeating the effect. A verified write
  may advance a sole write stage but cannot itself close the tool surface or
  terminate the task. With multiple write stages, leave completion attribution
  to model checkpoints rather than reusing one unbound write for later stages.
  The model may finish directly from returned results while advisory stages
  remain unacknowledged, or continue remaining deliverables with tools available.
  This is
  especially important for visual table transcription: carry full rows directly
  in closed structured write arguments instead of narrating an OCR transcript or
  reloading the same image solely to reconstruct those arguments.
  Never reject an image-stage completion merely because the same response lacks
  prose extraction or an immediate consumer. Preserve selected established
  results across the advisory boundary; use ordinary outcome recovery if the
  model later stops before the required verified effect.
  If an image-backed structured write fails validation after carrying the full
  table, persist that exact failed call and diagnostic as a bounded untrusted
  repair checkpoint. Keep the current-request source available during repair;
  arbitrary model prose or a checkpoint declaration cannot replace source bytes.
  The checkpoint cannot grant authority, prove correctness or satisfy a stage.
  For transcription and extraction, source fidelity outranks arithmetic repair.
  Arithmetic is a diagnostic for a possible visual misread, not authority to
  change source data. Recheck only the disputed cell or cells once; if the value
  remains legible but inconsistent, preserve it exactly, note the anomaly
  concisely and continue unless the user explicitly requested correction. Never
  silently repair a source inconsistency.
  For legacy typed contracts, a read step completes only when a trusted operation
  receipt reaches its declared evidence strength.
  Preserve reusable receipts, consumed budgets and verified ledger facts across
  revision; reopen effectful completion when its outcome binding changes, and
  acknowledge an identical semantic revision without consuming capacity.

## Evidence tools and diagnostics

- Keep compound evidence selection structurally obvious. `find_matches` is a
  discovery operation and requires a non-empty filename fragment, one or more
  extensions, or both, with `path` describing only its search root. Every
  `load_exact_*` operation instead requires one already-known exact relative
  file path and accepts no discovery selector fields. Do not require a redundant
  filename or extension beside an exact path. Describe these modes separately,
  return concise path-specific validation errors, and remove legacy obligation
  language from the v6-facing collector description. State explicitly that the
  collector's top level contains only `workspace` and `items`, with `path` nested
  inside each item, and include one canonical example for each mode. Treat
  `complete` on `filesystem.find_paths` only as exhaustive-coverage metadata: a
  returned `relativePath` remains usable when `complete=false`, so the model must
  not rediscover an already-sufficient candidate merely because broader coverage
  was bounded.
- Keep argument corrections exact and compact. For an ordinary call, return the
  operation, first failing JSON path, expected type/enum or permitted fields,
  semantic detail and instruction to preserve unrelated valid arguments. For a
  structured call with independent row/cell defects, return one bounded defect
  set instead. Do not duplicate an entire advertised schema into retained
  history; use one canonical example in the tool description when nesting is
  otherwise easy to misunderstand.
- Make long local inference observable without logging model content. Development
  telemetry must record round start after prompt admission, the first streamed
  delta, and bounded periodic stream-progress events containing only elapsed time,
  delta count and channel class. Completion telemetry remains authoritative for
  final token and timing metrics; absence of a completion record after a start is
  not by itself evidence that a visibly streaming run has stalled.
- For legacy typed plans, represent evidence strength and purpose as closed
  host-validated data when a later effect depends on them. Keep identity/location,
  format/structure and
  semantic-content evidence distinct; path discovery cannot establish loaded
  structure or meaning. Bind effects to exact completed evidence receipts rather
  than inferring readiness from a tool name, conversational prose or stage count.
  Such provenance proves what operation supplied a declared role, not that
  untrusted model interpretation is semantically correct.

## Checkpoints and host phases

- Pin accepted plans to a contract and policy version. Preserve a legacy parser
  for already-active plans during migration, but do not silently reinterpret one
  version as another. Exercise orchestration policies with dissimilar tools and
  outcomes, using a golden workflow as one trajectory rather than as the
  implementation's source of task-specific rules.
- At every model round, inject a compact host-owned phase directive derived from
  the current semantic stage and executable catalogue. Include the current stage
  objective, completion condition, dependencies, promoted suggestions, a direct
  action instruction, a searchable category index, and the allowed and required
  top-level arguments for relevant tools. Relevant-tool summaries guide selection;
  they do not hide other available v6 task tools. Generate this JSON from the same
  definitions used by validation so reminder text cannot drift into a second tool
  contract.
- After a plan is accepted, build the inference context as a host-owned phase
  capsule rather than replaying the full conversation. Include the original user
  instruction, accepted plan summary, current stage objective and completion
  condition, suggested tools, useful prior results and the authoritative ledger.
  Do not expose per-obligation receipt accounting in v6. Retain only the smallest
  result context needed for the current stage and verified outcome recovery.
  When every accepted stage is complete and no current stage remains, select
  established results across the completed plan for final answering. An empty
  execution frontier is not an empty evidence set: retain paired calls/results,
  source URLs, uncertainty and coverage metadata, plus existing revision and
  unresolved-failure context. Preserve a bounded chronological dialogue spine
  containing the retained user messages and user-visible assistant messages they
  answer, alongside the accepted target, constraints and success criteria. Keep
  exact current inputs while admission permits; compact older context intentionally
  with decisions, identifiers, outcomes, blockers and exact scoped recall. Never
  treat the latest user message as sufficient multi-turn state. A generation reset,
  plan revision or normal assistant stop is not a dialogue truncation boundary.
  Use typed compaction and exact admission; expose an unrecoverable context gap
  rather than silently discarding necessary inputs. Do not change completion or
  tool authority.
  Cover this boundary with text-only research as well as artefact workflows. Make
  capsule assembly deterministic, digest-bound and subject to exact prompt-token
  admission. Preserve useful results across plan revision.
  Exclude private reasoning from generic semantic summaries and factual task
  state. Preserve provider-required continuation in its native adapter channel
  according to the tested model/runtime contract; never turn it into visible
  dialogue or a receipt. Exclude superseded attempts and unrelated evidence from
  automatic replay, while retaining actionable repair context and scoped recall.
  Preserve assistant/tool call linkage for selected results, but project verbose
  discovery and aggregate-evidence results through deterministic typed compaction:
  retain selected paths, coverage/truncation, exact loaded text, image provenance,
  verified outcomes and unresolved failures while removing repeated selectors,
  traversal counters and explanatory fields already present in the paired call or
  active plan. Retain a raw result only until it has a compact task-state
  representation with usable scoped recovery. A downstream structured call may
  preserve derived values but does not prove that source evidence is unnecessary
  or correctly interpreted. Never treat prompt caching as a substitute for context
  reduction, or a retained reference as proof that its payload is still available.
  When retaining an image-derived continuation, retain any completed tool call
  emitted by that same assistant response together with its matching result.
  Never project a reasoning-only remnant created by dropping one side of a
  completed call/result pair; provider adapters may reject that malformed
  history, and the result may describe a newly loaded capability needed next.
  If exact admission fails within one long user turn, omit older completed
  exchanges only as paired batches and only with available scoped recovery.
  Preserve selected user dialogue, the current accepted plan, recent native
  batches and the latest image producer. Try this projection before advancing
  the dialogue checkpoint; do not silently discard user inputs to fit derived
  tool history. The recovery decision and native failure are recorded in
  the orchestration recovery pass.
- Represent execution as a closed host state: planning, current-step execution,
  same-step repair, bounded refinement, plan revision, awaiting-user,
  verification and completion. Derive a dependency-ready frontier from the
  semantic graph. Focus the capsule on one primary step, permit batching of
  independent ready steps, and never credit a downstream step before all of its
  declared dependencies complete. A refinement is depth-first child work and
  returns to its suspended parent when its concrete gap is resolved.

## Reasoning budgets and recovery

- Treat private reasoning as capacity spent, not visible progress. Ask the model
  to think only enough to choose and form the next action; do not reward task,
  plan, schema, evidence or argument restatement. Map latency-oriented work to the
  appropriate provider control: Quick mode retains Qwen's explicit hard disable;
  native low-effort thinking remains a distinct adapter option. Semantic planning
  still runs through structured tools with thinking disabled. Balanced and Deep
  effort retain their native overrides.
  Adapters without a hard switch use a supported low directive and retain a
  strict total generation ceiling. Use publisher sampling for the selected mode.
  Select complete-artefact argument capacity independently of reasoning effort:
  when an exposed tool accepts bulk file content or table rows, permit the
  configured product output ceiling, including during repairs. Do not truncate
  whole-artefact calls with a small generic mutation profile. Exact context,
  time, call, output and permission limits still apply; partial calls never run.
- Do not equate accumulated corrections with semantic ambiguity. Corrections may
  justify a balanced bounded profile, but deep reasoning requires a current
  ambiguity in the task evidence or an explicit reasoning-capable recovery.
  An explicit image reinspection is also a transport fact, not an ambiguity
  judgment: routine initial inspection and reinspection both use balanced
  reasoning. The pinned Qwen3.8 renderer prepends xhigh instructions before tools
  for high reasoning, whereas medium and disabled thinking share that prefix.
  Test transitions against provider-rendered prompts, not only model-neutral
  messages, and keep deliberate high reasoning available.
- Permit bounded recovery only when the host can focus the next attempt on the
  failed action. A low-reasoning round that exhausts before action may unlock a
  reasoning-capable recovery from persisted state. Allow up to three attempts for
  the same repairable diagnostic and cap total corrections independently; after
  that, change strategy through a concrete plan revision or clarification rather
  than replaying an equivalent full-context inference.
- Canonicalise syntactically distinct representations only when they are provably
  authority-equivalent, such as an omitted optional evidence list and an empty
  list that grants no role. Never use normalisation to guess content, targets,
  permissions or effectful intent. Distinguish same-step independent duplication
  from cross-step dependency before rejecting a planned operation as redundant.
- Enforce hard context, time, output, call and capability limits in the runtime.
  Derive a soft working allowance from semantic step count and preferred operations,
  with bounded
  headroom, while retaining global ceilings as host-owned safety limits. A useful
  completed-but-incomplete evidence result may add bounded refinement headroom;
  rejection and transport failure may not. Project the remaining adaptive
  allowance explicitly; do not make the model infer it from a larger global
  ceiling.
- Use the minimum verification necessary to establish the requested outcome.
  Treat the requested result and its success criteria as the scope boundary.
  Do not expand into adjacent analysis, auditing, optimisation or stronger
  assurance unless the user requested it or a concrete evidence gap prevents
  reliable completion. A necessary expansion revises the unfinished plan,
  identifies the gap and uses the smallest targeted check that resolves it.

## Evidence reuse and batch repair

- Classify evidence by the decision it supports: identity and location, format
  and structure, or semantic content. Request the weakest class capable of
  resolving the concrete outcome dependency. Prefer authoritative metadata,
  manifests and schemas over representative content when they answer the same
  question. Keep stronger example-content and layout inspection available when
  compatibility or meaning genuinely depends on it, but require that unresolved
  requirement to be explicit and retain the class in content-free diagnostics.
- Consolidate independent selectors supported by one batch-capable evidence tool
  into one semantic step and one operation. Permit sequential reuse when a later
  exact read depends on candidates returned by an earlier discovery step; this is
  dependency execution, not redundant discovery. A safe alternative operation may
  advance the current objective without revision. Require revision only when the
  deliverable, target scope, dependency order, success criteria or action-risk
  class changes materially.
- Reuse only when exact scope, arguments and live source state prove equivalence.
  Treat overlapping or near-duplicate work as an advisory optimisation unless a
  typed, complete coverage relation proves that an existing observation answers
  it. Incomplete or truncated discovery permits refinement; mutations invalidate
  affected read evidence.
- Cache exact, non-retryable read failures within the request revision as well as
  successful observations. Replaying the same deterministic failure should return
  its compact sanitised result; transient failures and mutations remain eligible
  for normal execution policy.
  Cache failures only from an executed operation with Failed activity status.
  Partial scheduling/admission results and cancellation are never deterministic
  observations and never advance the useful-result checkpoint. Distinguish a
  deferred operation from a completed read whose output was withheld; provide
  bounded later-batch retry guidance without replaying writes or resetting the
  request ceilings.
- A batch with exactly one safely repairable malformed call may preserve valid
  siblings only when every call is a known read-only operation and plan admission
  succeeds first. Mutation-bearing, unknown, unsafe or multiply malformed batches
  remain atomic and execute nothing. Separately, a bounded batch rejected solely
  because its known operations are unavailable in the current phase receives one
  paired error result per call and continues through bounded recovery; rejecting
  effects does not by itself terminate the model loop. Unknown calls receive the
  same treatment only when every call envelope is safely bounded. Unsafe or
  unrepresentable envelopes still fail closed.
- A valid non-effecting plan control followed by exactly one malformed known
  read-only operation may likewise preserve the accepted plan while executing no
  malformed operation and returning one bounded schema correction. The malformed
  call does not advance forecast progress or adaptive execution allowance, but its
  model call and correction attempt remain charged to the global ceilings. Validate
  the plan independently first; invalid plans, mutations, unknown tools and repeated
  malformed calls still fail closed.
- Project dynamic control schemas from authoritative host state. Advertise only
  control variants legal in the current state; do not ask the model to remember a
  state transition that the schema still contradicts. Normalise a stale,
  authority-equivalent control idempotently before charging correction or revision
  capacity.
- Prefer one closed discriminator when fields jointly determine behaviour or
  authority. Independent enum fields are inappropriate when only a subset of
  their Cartesian product is meaningful; make invalid combinations
  unrepresentable in the model-visible schema and derive internal facets from the
  discriminator.
- Validate control calls and operation calls independently before deciding batch
  fate. A rejected non-effecting control must not erase independently valid,
  current-plan read-only work. Preserve only fully validated safe reads; mutations,
  unknown calls and scope deviations remain atomic and fail closed.

## Completion and verified outcomes

- Gate completion on the requested outcome and its verified postcondition, not an
  intermediate operation. Creating a parent directory cannot complete a request
  to create a file.
- Project plan progress from trusted execution facts after each batch. Transport
  success is not plan progress: advance an evidence step only when its typed
  result establishes the operation's completion condition. A multi-selector
  discovery is unresolved while any selector has neither a candidate nor
  exhaustive coverage; an incomplete collector remains on the same refinable
  step. Never let the call's mere completion move the active capability cursor.
  Match forecast calls against the current unfinished step rather than crediting
  a same-named operation in a later step. When one tool is forecast against
  different targets, use one bounded exact argument discriminator so a call only
  advances its intended target. Retain declared purposes and discriminator values
  for matching and diagnostics without reinjecting their free-form or target
  content. Project deviation counts, typed
  outcome status and registry-validated remaining tool names. Require a concrete
  evidence gap before adding checks or analysis outside unfinished forecast work.
  Permit one explicitly bounded recovery generation when a typed mutation outcome
  remains unsatisfied.
- Treat phase narrowing as a focus mechanism, not a read-only dead end. When a
  model requests a globally advertised, non-effecting operation after a stale
  cursor transition, reopen matching suspended forecast work. If no prior call
  matches, grant one bounded safe-read escape, retain the deviation in trusted
  progress, and require the next decision to return to the plan or revise it.
  Keep the safe capability visible for that plan revision after its one-result
  bypass is consumed so further misuse receives a scope correction, never a
  contradictory unavailable-capability failure.
  Validate arguments normally and keep mutations fail-closed. An unavailable-tool
  correction loop is never an acceptable terminal path for known safe reads.
- Use strict tool routing only for a host-proven bounded stage, never merely
  because a plan prefers one tool. Require an active dependency-ready step, one
  exact action-compatible advertised operation, trusted dependency receipts that
  ground its inputs and a closed broker-verifiable postcondition. State plainly
  that this proves structural readiness and provenance, not the semantic truth of
  untrusted evidence. Keep plan revision and a concise user clarification as
  explicit exits; an invalid argument, unavailable operation, failed execution or
  missing postcondition must restore an adaptive legal route rather than trap the
  request. Credit progress only after the ordinary permission broker reports the
  declared postcondition.
- Treat recovery as a typed request-local checkpoint, not a fresh task. A
  repairable validation, phase, provider, tool or output failure resumes the same
  semantic step while retaining the accepted plan, useful receipts, operation
  ledger, verified mutations, consumed allowances and bounded-stage state that
  remain valid. Advance the checkpoint only for a material plan revision or new
  useful tool result; changing prompt prose does not create progress.
- Permit up to three repairs for one typed diagnostic only when arguments,
  evidence, execution mode, scope or typed external failure state changes in a
  way that can affect the outcome. Suppress exact operations through the
  authoritative ledger and reject an identical non-transient recovery inference
  before paying provider latency. Terminal recovery must identify the closed
  boundary and the concrete change that could permit continuation. Cancellation
  remains immediate and must not fabricate completion or discard already
  persisted history.
- Mutating results must report host-verified postconditions appropriate to the
  generic tool. Do not imply format-specific or semantic validation from a byte,
  digest or atomicity check.
- Evaluate task correctness, evidence and postconditions before optimising calls,
  turns, latency, tokens or cost. Preserve representative failure traces as eval
  baselines and change one orchestration layer at a time when measuring causality.
