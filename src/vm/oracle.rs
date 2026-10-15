//! The pinned `wasmi` engine as a comparison oracle, behind the `oracle`
//! feature. It runs the instrumented form of a module (private debit imports
//! and indirect-call thunks, see `instrument`) through the same host layer as
//! the reference interpreter, so the two can be run side by side on the same
//! inputs. It is not part of the consensus build.
use super::{
    host::{Host, HostResult, Interrupt, Memory},
    instrument::instrument,
    price::{self, Tier},
    profile::Profile,
    runtime::Progress,
};
use crate::{meter::Resources, protocol::Status};
use std::{fmt, rc::Rc};
use wasmi::{
    Caller, Config, Engine, Linker, Module, ResourceLimiter, Store, TypedFunc, TypedResumableCall,
    TypedResumableCallHostTrap, Val,
};
use wasmi_core::{HostError, LimiterError};

type WResult<T> = Result<T, wasmi::Error>;

#[derive(Clone)]
pub(super) struct Backend {
    engine: Engine,
}
pub(super) struct Prepared {
    module: Module,
    /// The entry function's reserved slots.
    entry: u64,
}

/// A host request that suspends the guest until the driver has run the child.
#[derive(Debug)]
struct Suspend;
impl fmt::Display for Suspend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ting nested invocation")
    }
}
impl HostError for Suspend {}

struct Data {
    host: Host,
    initializing: bool,
    growth_allowed: bool,
    /// The stack's height and mark in slots, restated from the wiki's
    /// function-entry rule (vm/metering) independently of the interpreter.
    height: u64,
    mark: u64,
}
/// Slots per 64 KiB of stack, and per active function's record.
const STACK_SLOTS: u64 = 8_192;
const RECORD_SLOTS: u64 = 3;
/// Started 64 KiB of a stack `height` slots high, beyond the first.
fn paid_pages(height: u64) -> u64 {
    if height <= STACK_SLOTS {
        0
    } else {
        (height - 1) / STACK_SLOTS
    }
}

impl Backend {
    pub fn new() -> Self {
        let mut config = Config::default();
        config
            .consume_fuel(false)
            .floats(false)
            .allow_start_fn(false)
            .wasm_multi_value(false)
            .wasm_multi_memory(false)
            .wasm_bulk_memory(true)
            .wasm_reference_types(false)
            .wasm_tail_call(false)
            .wasm_extended_const(false)
            .wasm_sign_extension(true)
            .wasm_saturating_float_to_int(false)
            .wasm_custom_page_sizes(false)
            .wasm_wide_arithmetic(true);
        // Guest recursion is bounded by compute, never by the backend.
        config
            .set_max_recursion_depth(usize::MAX >> 1)
            .set_max_stack_height(usize::MAX >> 1);
        Self {
            engine: Engine::new(&config),
        }
    }
    pub fn prepare(&self, code: &[u8], profile: &Profile) -> Rc<Prepared> {
        let executable =
            instrument(code, profile).expect("validated module instrumentation must succeed");
        let module = Module::new(&self.engine, &executable)
            .expect("backend must accept the frozen validated profile");
        let entry = profile.functions[profile.invoke as usize].slots();
        Rc::new(Prepared { module, entry })
    }
}

pub(super) struct Instance {
    store: Store<Data>,
    invoke: TypedFunc<(), ()>,
    suspended: Option<TypedResumableCallHostTrap<()>>,
}

impl Instance {
    pub fn new(backend: &Backend, prepared: &Prepared, host: Host) -> Self {
        let mut store = Store::new(
            &backend.engine,
            Data {
                host,
                initializing: true,
                growth_allowed: false,
                // The runtime paid the entry function's reservation and pages.
                height: prepared.entry + RECORD_SLOTS,
                mark: prepared.entry + RECORD_SLOTS,
            },
        );
        store.limiter(|data| data);
        let linker = linker(&backend.engine);
        let instance = match linker.instantiate_and_start(&mut store, &prepared.module) {
            Ok(instance) => instance,
            Err(error) => panic!("backend failed validated instance preparation: {error}"),
        };
        store.data_mut().initializing = false;
        let invoke = instance
            .get_typed_func::<(), ()>(&store, "invoke")
            .expect("validated invoke export");
        Self {
            store,
            invoke,
            suspended: None,
        }
    }
    pub fn host(&self) -> &Host {
        &self.store.data().host
    }
    pub fn host_mut(&mut self) -> &mut Host {
        &mut self.store.data_mut().host
    }
    pub fn start(&mut self) -> Progress {
        let result = self.invoke.call_resumable(&mut self.store, ());
        self.settle(result)
    }
    pub fn resume(&mut self, status: i32) -> Progress {
        let invocation = self.suspended.take().expect("a suspended invocation");
        let result = invocation.resume(&mut self.store, &[Val::I32(status)]);
        self.settle(result)
    }
    fn settle(&mut self, result: WResult<TypedResumableCall<()>>) -> Progress {
        let host = &self.store.data().host;
        match result {
            Ok(TypedResumableCall::Finished(())) => Progress::Finished,
            Ok(TypedResumableCall::HostTrap(invocation)) => {
                if host.outcome.is_some() {
                    Progress::Interrupted
                } else if host.pending.is_some() {
                    self.suspended = Some(invocation);
                    Progress::Interrupted
                } else {
                    panic!(
                        "host failed without a specified guest outcome: {}",
                        invocation.host_error()
                    );
                }
            }
            Ok(TypedResumableCall::OutOfFuel(_)) => panic!("fuel metering is not enabled"),
            Err(error) => {
                if host.outcome.is_some() {
                    return Progress::Interrupted;
                }
                match error.as_trap_code() {
                    Some(
                        wasmi::TrapCode::UnreachableCodeReached
                        | wasmi::TrapCode::IntegerDivisionByZero
                        | wasmi::TrapCode::IntegerOverflow
                        | wasmi::TrapCode::MemoryOutOfBounds
                        | wasmi::TrapCode::TableOutOfBounds
                        | wasmi::TrapCode::IndirectCallToNull
                        | wasmi::TrapCode::BadSignature,
                    ) => Progress::Trapped,
                    _ => panic!("backend failed before a specified guest outcome: {error}"),
                }
            }
        }
    }
}

impl ResourceLimiter for Data {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, LimiterError> {
        self.growth_allowed = false;
        let delta = desired.saturating_sub(current);
        if maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let pages = delta as u64 / super::decode::PAGE;
        if !self.initializing
            && let Err(status) = self
                .host
                .runtime
                .charge(Resources::compute(price::PAGE * pages))
        {
            self.host.outcome = Some((status, Vec::new()));
            return Err(LimiterError::ResourceLimiterDeniedAllocation);
        }
        self.growth_allowed = true;
        Ok(true)
    }
    fn memory_grow_failed(
        &mut self,
        _error: &wasmi::errors::MemoryError,
    ) -> Result<(), LimiterError> {
        if self.growth_allowed {
            panic!("host failed permitted deterministic memory allocation");
        }
        Ok(())
    }
    /// The table is allocated once at instantiation and never grows: the profile
    /// has no table instructions, and its entries are paid in the preparation debit.
    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, LimiterError> {
        Ok(self.initializing && maximum.is_none_or(|max| desired <= max))
    }
    fn instances(&self) -> usize {
        1
    }
    fn tables(&self) -> usize {
        1
    }
    fn memories(&self) -> usize {
        1
    }
}

/// Runs one host import against the exported memory, translating an interrupt
/// into the backend's resumable or terminal error.
fn with_host<T>(
    caller: &mut Caller<'_, Data>,
    f: impl FnOnce(&mut Host, &mut Memory<'_>) -> HostResult<T>,
) -> WResult<T> {
    let memory = caller.get_export("memory").and_then(|e| e.into_memory());
    let result = match memory {
        Some(memory) => {
            let (bytes, data) = memory.data_and_store_mut(&mut *caller);
            f(&mut data.host, &mut Some(bytes))
        }
        None => f(&mut caller.data_mut().host, &mut None),
    };
    result.map_err(|Interrupt| interrupt(caller))
}
/// The tier of the instance's current memory size.
fn tier(caller: &Caller<'_, Data>) -> Tier {
    let bytes = caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .map_or(0, |m| m.data(caller).len() as u64);
    Tier::of(bytes)
}
fn interrupt(caller: &mut Caller<'_, Data>) -> wasmi::Error {
    if caller.data().host.pending.is_some() {
        wasmi::Error::host(Suspend)
    } else {
        wasmi::Error::new("Ting invocation ended")
    }
}

fn linker(engine: &Engine) -> Linker<Data> {
    let mut l = Linker::<Data>::new(engine);
    l.func_wrap(
        "__ting_meter_v1",
        "charge",
        |mut c: Caller<'_, Data>, n: i64| {
            let result = {
                let data = c.data_mut();
                data.growth_allowed = false;
                data.host.charge(Resources::compute(n as u64))
            };
            result.map_err(|Interrupt| interrupt(&mut c))
        },
    )
    .unwrap();
    l.func_wrap(
        "__ting_meter_v1",
        "enter",
        |mut c: Caller<'_, Data>, slots: i64| {
            let result = {
                let data = c.data_mut();
                data.growth_allowed = false;
                let height = data.height + slots as u64 + RECORD_SLOTS;
                let mark = data.mark.max(height);
                let pages = paid_pages(mark) - paid_pages(data.mark);
                let charged = data
                    .host
                    .charge(Resources::compute(slots as u64 + 40_000 * pages));
                if charged.is_ok() {
                    (data.height, data.mark) = (height, mark);
                }
                charged
            };
            result.map_err(|Interrupt| interrupt(&mut c))
        },
    )
    .unwrap();
    l.func_wrap(
        "__ting_meter_v1",
        "leave",
        |mut c: Caller<'_, Data>, slots: i64| {
            let data = c.data_mut();
            data.height -= slots as u64 + RECORD_SLOTS;
        },
    )
    .unwrap();
    l.func_wrap(
        "__ting_meter_v1",
        "bytes",
        |mut c: Caller<'_, Data>, n: i32| {
            let bytes = tier(&c).bytes(u64::from(n as u32));
            let result = {
                let data = c.data_mut();
                data.growth_allowed = false;
                data.host.charge(Resources::compute(bytes))
            };
            result.map_err(|Interrupt| interrupt(&mut c))
        },
    )
    .unwrap();
    l.func_wrap(
        "__ting_meter_v1",
        "access",
        |mut c: Caller<'_, Data>, kind: i32| {
            let tier = tier(&c);
            let cost = if kind == 0 { tier.load } else { tier.store };
            let result = {
                let data = c.data_mut();
                data.growth_allowed = false;
                data.host.charge(Resources::compute(cost))
            };
            result.map_err(|Interrupt| interrupt(&mut c))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "input",
        |mut c: Caller<'_, Data>, ptr: i32, cap: i32| {
            with_host(&mut c, |h, m| h.input(m, ptr, cap))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "call_result",
        |mut c: Caller<'_, Data>, ptr: i32, cap: i32| {
            with_host(&mut c, |h, m| h.call_result(m, ptr, cap))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "context",
        |mut c: Caller<'_, Data>, kind: i32, ptr: i32, cap: i32| {
            with_host(&mut c, |h, m| h.context(m, kind, ptr, cap))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "storage_get",
        |mut c: Caller<'_, Data>, key: i32, ptr: i32, cap: i32| {
            with_host(&mut c, |h, m| h.storage_get(m, key, ptr, cap))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "code_hash",
        |mut c: Caller<'_, Data>, target: i32, ptr: i32, cap: i32| {
            with_host(&mut c, |h, m| h.code_hash(m, target, ptr, cap))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "storage_set",
        |mut c: Caller<'_, Data>, key: i32, value: i32, len: i32| {
            with_host(&mut c, |h, m| h.storage_set(m, key, value, len))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "storage_delete",
        |mut c: Caller<'_, Data>, key: i32| with_host(&mut c, |h, m| h.storage_delete(m, key)),
    )
    .unwrap();
    for (name, status) in [("return", Status::Success), ("revert", Status::Revert)] {
        l.func_wrap(
            "ting",
            name,
            move |mut c: Caller<'_, Data>, ptr: i32, len: i32| {
                with_host(&mut c, |h, m| h.finish(m, status, ptr, len))
            },
        )
        .unwrap();
    }
    l.func_wrap(
        "ting",
        "emit",
        |mut c: Caller<'_, Data>, ptr: i32, len: i32| with_host(&mut c, |h, m| h.emit(m, ptr, len)),
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "call",
        |mut c: Caller<'_, Data>, target: i32, ptr: i32, len: i32, budget: i32, flags: i32| {
            with_host(&mut c, |h, m| h.call(m, target, ptr, len, budget, flags))
        },
    )
    .unwrap();
    l.func_wrap(
        "ting",
        "deploy",
        |mut c: Caller<'_, Data>, code: i32, code_len: i32, ptr: i32, len: i32, budget: i32| {
            with_host(&mut c, |h, m| h.deploy(m, code, code_len, ptr, len, budget))
        },
    )
    .unwrap();
    l
}
