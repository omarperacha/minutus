use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

use rquickjs::{Context, Ctx, Exception, Function, Module, Object, Runtime, TypedArray, Value};

use crate::contract::{
    ComputationOutput, ComputationRequest, InputFormat, OutputKind, RunnerStatus,
    validate_json_bounds,
};
use crate::resource_policy::{
    CPU_TIME_LIMIT, JAVASCRIPT_HEAP_LIMIT_BYTES, JAVASCRIPT_STACK_LIMIT_BYTES,
    PROCESS_MEMORY_LIMIT_BYTES, WALL_TIME_LIMIT, process_usage,
};

const INTERRUPTED_NONE: u8 = 0;
const INTERRUPTED_WALL: u8 = 1;
const INTERRUPTED_CPU: u8 = 2;
const INTERRUPTED_MEMORY: u8 = 3;
const INTERRUPTED_POLICY: u8 = 4;

#[cfg(test)]
mod tests;

// Capture intrinsics before evaluating untrusted source. JSON.stringify normally
// collapses NaN/infinity to null, which would falsely report a valid result.
// Number.valueOf also recognizes boxed numbers without trusting toStringTag.
const JSON_OUTPUT_REPLACER: &str = r#"
(() => {
  const apply = Reflect.apply;
  const numberValueOf = Number.prototype.valueOf;
  const finite = Number.isFinite;
  const Failure = TypeError;
  return function (_key, value) {
    let number = value;
    if (value !== null && typeof value === "object") {
      try { number = apply(numberValueOf, value, []); }
      catch { return value; }
    }
    if (typeof number === "number" && !finite(number)) {
      throw new Failure("non-finite JSON computation output");
    }
    return value;
  };
})()
"#;

const BOOTSTRAP: &str = r#"
delete globalThis.console;
delete globalThis.Date;
delete globalThis.SharedArrayBuffer;
delete globalThis.Atomics;
delete globalThis.WebAssembly;
Object.defineProperty(Math, "random", {
  value() { throw new TypeError("randomness is unavailable"); },
  writable: false,
  configurable: false
});
Object.freeze(Math);
Object.freeze(globalThis.__zeta);
"#;

#[derive(Clone)]
struct DecodedInput {
    format: InputFormat,
    bytes: Vec<u8>,
}

pub(crate) fn execute(request: &ComputationRequest) -> crate::ComputationResponse {
    let request_id = request.request_id().to_owned();
    match execute_inner(request) {
        Ok(output) => {
            crate::ComputationResponse::terminal(request_id, RunnerStatus::Completed, Some(output))
        }
        Err(status) => crate::ComputationResponse::terminal(request_id, status, None),
    }
}

fn execute_inner(request: &ComputationRequest) -> Result<ComputationOutput, RunnerStatus> {
    request
        .validate()
        .map_err(|()| RunnerStatus::InvalidRequest)?;
    let inputs = Arc::new(decode_inputs(request)?);
    let deadline = Instant::now() + WALL_TIME_LIMIT;
    let interrupted = Arc::new(AtomicU8::new(INTERRUPTED_NONE));
    let interrupt_state = Arc::clone(&interrupted);
    let runtime = Runtime::new().map_err(|error| runtime_error_status(&error))?;
    runtime.set_memory_limit(JAVASCRIPT_HEAP_LIMIT_BYTES);
    runtime.set_max_stack_size(JAVASCRIPT_STACK_LIMIT_BYTES);
    runtime.set_interrupt_handler(Some(Box::new(move || {
        let reason = match process_usage(std::process::id()) {
            Ok(usage) if usage.cpu_time >= CPU_TIME_LIMIT => INTERRUPTED_CPU,
            Ok(usage) if usage.physical_bytes > PROCESS_MEMORY_LIMIT_BYTES => INTERRUPTED_MEMORY,
            Ok(_) if Instant::now() >= deadline => INTERRUPTED_WALL,
            Ok(_) => INTERRUPTED_NONE,
            Err(()) => INTERRUPTED_POLICY,
        };
        if reason != INTERRUPTED_NONE {
            interrupt_state.store(reason, Ordering::Release);
        }
        reason != INTERRUPTED_NONE
    })));
    let context = Context::full(&runtime).map_err(|error| runtime_error_status(&error))?;
    let result = context.with(|ctx| execute_in_context(&ctx, request, inputs));
    match interrupted.load(Ordering::Acquire) {
        INTERRUPTED_WALL => Err(RunnerStatus::WallLimit),
        INTERRUPTED_CPU => Err(RunnerStatus::CpuLimit),
        INTERRUPTED_MEMORY => Err(RunnerStatus::MemoryLimit),
        INTERRUPTED_POLICY => Err(RunnerStatus::PolicyUnavailable),
        _ if runtime.is_job_pending() => Err(RunnerStatus::RuntimeFailed),
        _ => result,
    }
}

fn decode_inputs(
    request: &ComputationRequest,
) -> Result<BTreeMap<String, DecodedInput>, RunnerStatus> {
    request
        .inputs()
        .iter()
        .map(|input| {
            Ok((
                input.id().to_owned(),
                DecodedInput {
                    format: input.format(),
                    bytes: input.decode().map_err(|()| RunnerStatus::InvalidRequest)?,
                },
            ))
        })
        .collect()
}

fn execute_in_context(
    ctx: &Ctx<'_>,
    request: &ComputationRequest,
    inputs: Arc<BTreeMap<String, DecodedInput>>,
) -> Result<ComputationOutput, RunnerStatus> {
    install_zeta_api(ctx, inputs).map_err(|error| runtime_error_status(&error))?;
    ctx.eval::<(), _>(BOOTSTRAP)
        .map_err(|error| runtime_error_status(&error))?;
    let json_replacer: Function = ctx
        .eval(JSON_OUTPUT_REPLACER)
        .map_err(|error| runtime_error_status(&error))?;

    let module =
        Module::declare(ctx.clone(), "zeta:computation", request.source()).map_err(|error| {
            match error {
                rquickjs::Error::Allocation => RunnerStatus::MemoryLimit,
                _ => RunnerStatus::CompileFailed,
            }
        })?;
    let (module, evaluated) = module
        .eval()
        .map_err(|error| runtime_error_status(&error))?;
    evaluated
        .finish::<()>()
        .map_err(|error| runtime_error_status(&error))?;
    let compute: Function = module.get("default").map_err(|error| match error {
        rquickjs::Error::Allocation => RunnerStatus::MemoryLimit,
        _ => RunnerStatus::CompileFailed,
    })?;
    let zeta: Object = ctx
        .globals()
        .get("__zeta")
        .map_err(|error| runtime_error_status(&error))?;
    let value: Value = compute
        .call((zeta,))
        .map_err(|error| contextual_runtime_error_status(ctx, &error))?;
    if value.as_promise().is_some() {
        return Err(RunnerStatus::RuntimeFailed);
    }
    encode_output(ctx, request, value, json_replacer)
}

fn runtime_error_status(error: &rquickjs::Error) -> RunnerStatus {
    if matches!(error, rquickjs::Error::Allocation) {
        RunnerStatus::MemoryLimit
    } else {
        RunnerStatus::RuntimeFailed
    }
}

fn contextual_runtime_error_status(ctx: &Ctx<'_>, error: &rquickjs::Error) -> RunnerStatus {
    if matches!(error, rquickjs::Error::Allocation) {
        return RunnerStatus::MemoryLimit;
    }
    if matches!(error, rquickjs::Error::Exception)
        && ctx
            .catch()
            .into_exception()
            .and_then(|exception| exception.message())
            .is_some_and(|message| message.eq_ignore_ascii_case("out of memory"))
    {
        RunnerStatus::MemoryLimit
    } else {
        RunnerStatus::RuntimeFailed
    }
}

fn install_zeta_api<'js>(
    ctx: &Ctx<'js>,
    inputs: Arc<BTreeMap<String, DecodedInput>>,
) -> rquickjs::Result<()> {
    let zeta = Object::new(ctx.clone())?;

    let text_inputs = Arc::clone(&inputs);
    zeta.set(
        "inputText",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: String| {
            let input = require_input(&ctx, &text_inputs, &id, InputFormat::Utf8)?;
            String::from_utf8(input.bytes.clone())
                .map_err(|_| Exception::throw_type(&ctx, "input is not valid UTF-8"))
        })?,
    )?;

    let json_inputs = Arc::clone(&inputs);
    zeta.set(
        "inputJson",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: String| {
            let input = require_input(&ctx, &json_inputs, &id, InputFormat::Json)?;
            ctx.json_parse(input.bytes.clone())
        })?,
    )?;

    zeta.set(
        "inputBytes",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, id: String| {
            let input = require_input(&ctx, &inputs, &id, InputFormat::Bytes)?;
            TypedArray::<u8>::new_copy(ctx, &input.bytes)
        })?,
    )?;
    ctx.globals().set("__zeta", zeta)
}

fn require_input<'a>(
    ctx: &Ctx<'_>,
    inputs: &'a BTreeMap<String, DecodedInput>,
    id: &str,
    format: InputFormat,
) -> rquickjs::Result<&'a DecodedInput> {
    let input = inputs
        .get(id)
        .ok_or_else(|| Exception::throw_type(ctx, "unknown computation input"))?;
    if input.format != format {
        return Err(Exception::throw_type(
            ctx,
            "computation input format mismatch",
        ));
    }
    Ok(input)
}

fn encode_output<'js>(
    ctx: &Ctx<'js>,
    request: &ComputationRequest,
    value: Value<'js>,
    json_replacer: Function<'js>,
) -> Result<ComputationOutput, RunnerStatus> {
    match request.output().kind() {
        OutputKind::Text => {
            let text = value
                .into_string()
                .ok_or(RunnerStatus::RuntimeFailed)?
                .to_string()
                .map_err(|_| RunnerStatus::RuntimeFailed)?;
            if text.len() > request.output().max_bytes() {
                return Err(RunnerStatus::OutputLimit);
            }
            Ok(ComputationOutput::Text(text))
        }
        OutputKind::Json => {
            let json = ctx
                .json_stringify_replacer(value, json_replacer)
                .map_err(|_| RunnerStatus::RuntimeFailed)?
                .ok_or(RunnerStatus::RuntimeFailed)?
                .to_string()
                .map_err(|_| RunnerStatus::RuntimeFailed)?;
            if json.len() > request.output().max_bytes() {
                return Err(RunnerStatus::OutputLimit);
            }
            let output = serde_json::from_str(&json).map_err(|_| RunnerStatus::RuntimeFailed)?;
            validate_json_bounds(&output, 32, request.output().max_items())
                .map_err(|()| RunnerStatus::OutputLimit)?;
            Ok(ComputationOutput::Json(output))
        }
    }
}
