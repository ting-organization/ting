//! The host side of an invocation, written once for every engine: identity,
//! input, the last child result, and the twelve imports (wiki: vm/host)
//! with their span checks, charges and failure order.
use super::{price::Tier, runtime::Runtime};
use crate::{
    hash::Hash,
    meter::Resources,
    protocol::{Event, Status},
    state::{manifest_key, record_key},
};
use std::{ops::Range, rc::Rc};

pub(super) type Outcome = (Status, Vec<u8>);

/// The guest must stop executing now. `Host::outcome` then holds a finished
/// invocation, or `Host::pending` a nested invocation to run before resuming.
#[derive(Debug)]
pub(super) struct Interrupt;
pub(super) type HostResult<T> = Result<T, Interrupt>;

/// A nested invocation requested by `call` or `deploy`.
pub(super) enum Pending {
    Call {
        target: Hash,
        input: Vec<u8>,
        readonly: bool,
    },
    Deploy {
        code: Vec<u8>,
        input: Vec<u8>,
    },
}

/// The guest's exported memory, when the module has one.
pub(super) type Memory<'a> = Option<&'a mut [u8]>;

pub(super) struct Host {
    pub runtime: Runtime,
    pub id: Hash,
    pub caller: Option<Hash>,
    pub readonly: bool,
    pub input: Rc<Vec<u8>>,
    pub last: Rc<Vec<u8>>,
    pub outcome: Option<Outcome>,
    pub pending: Option<Pending>,
}

/// The memory tier of the executing instance, which prices the bytes it moves.
fn tier(mem: &Memory<'_>) -> Tier {
    Tier::of(mem.as_deref().map_or(0, |m| m.len() as u64))
}

fn resource_bytes(bytes: &[u8]) -> Resources {
    Resources(std::array::from_fn(|i| {
        u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap())
    }))
}

impl Host {
    pub fn new(
        runtime: Runtime,
        id: Hash,
        caller: Option<Hash>,
        readonly: bool,
        input: Vec<u8>,
    ) -> Self {
        Self {
            runtime,
            id,
            caller,
            readonly,
            input: Rc::new(input),
            last: Rc::new(Vec::new()),
            outcome: None,
            pending: None,
        }
    }
    /// Ends the invocation with a specified guest outcome.
    pub fn stop<T>(&mut self, status: Status, result: Vec<u8>) -> HostResult<T> {
        self.outcome = Some((status, result));
        Err(Interrupt)
    }
    /// One atomic debit; exhaustion ends the invocation with its status.
    pub fn charge(&mut self, r: Resources) -> HostResult<()> {
        match self.runtime.charge(r) {
            Ok(()) => Ok(()),
            Err(status) => self.stop(status, Vec::new()),
        }
    }
    fn suspend<T>(&mut self, pending: Pending) -> HostResult<T> {
        self.pending = Some(pending);
        Err(Interrupt)
    }
    fn writable(&mut self) -> HostResult<()> {
        if self.readonly {
            self.stop(Status::ReadOnly, Vec::new())
        } else {
            Ok(())
        }
    }
    /// Validates one span against exported memory with mathematical addition
    /// of unsigned 32-bit values; a missing memory or an out-of-range end traps.
    fn span(&mut self, mem: &Memory<'_>, ptr: i32, len: i32) -> HostResult<Range<usize>> {
        let Some(mem) = mem else {
            return self.stop(Status::Trap, Vec::new());
        };
        let start = u64::from(ptr as u32);
        let end = start + u64::from(len as u32);
        if end <= mem.len() as u64 {
            Ok(start as usize..end as usize)
        } else {
            self.stop(Status::Trap, Vec::new())
        }
    }
    /// Validates every span, then debits the word-rate copy cost of all of them atomically.
    fn read_parts(&mut self, mem: &Memory<'_>, parts: &[(i32, i32)]) -> HostResult<Vec<Vec<u8>>> {
        let mut spans = Vec::with_capacity(parts.len());
        for &(ptr, len) in parts {
            spans.push(self.span(mem, ptr, len)?);
        }
        let total: u64 = spans.iter().map(|r| r.len() as u64).sum();
        self.charge(Resources::compute(tier(mem).bytes(total)))?;
        let mem = mem
            .as_deref()
            .expect("spans were validated against a memory");
        Ok(spans.into_iter().map(|r| mem[r].to_vec()).collect())
    }
    /// A getter's output span: the full capacity span is validated, and when
    /// a value of `len` bytes fits, its copy is debited and the start returned.
    /// `None` when it does not fit: nothing is charged and nothing is copied.
    fn output_span(
        &mut self,
        mem: &Memory<'_>,
        ptr: i32,
        cap: i32,
        len: usize,
    ) -> HostResult<Option<usize>> {
        let range = self.span(mem, ptr, cap)?;
        if range.len() < len {
            return Ok(None);
        }
        self.charge(Resources::compute(tier(mem).bytes(len as u64)))?;
        Ok(Some(range.start))
    }
    /// A getter's output: the bytes are copied only when they fit, after the
    /// debit, and the required length is returned either way.
    fn output(
        &mut self,
        mem: &mut Memory<'_>,
        ptr: i32,
        cap: i32,
        bytes: &[u8],
    ) -> HostResult<i32> {
        if let Some(start) = self.output_span(mem, ptr, cap, bytes.len())? {
            let mem = mem
                .as_deref_mut()
                .expect("span was validated against a memory");
            mem[start..start + bytes.len()].copy_from_slice(bytes);
        }
        Ok(bytes.len() as i32)
    }
    fn key(&self, bytes: &[u8]) -> Hash {
        record_key(2, self.id, bytes.try_into().unwrap())
    }

    pub fn input(&mut self, mem: &mut Memory<'_>, ptr: i32, cap: i32) -> HostResult<i32> {
        let data = self.input.clone();
        self.output(mem, ptr, cap, &data)
    }
    pub fn call_result(&mut self, mem: &mut Memory<'_>, ptr: i32, cap: i32) -> HostResult<i32> {
        let data = self.last.clone();
        self.output(mem, ptr, cap, &data)
    }
    pub fn context(
        &mut self,
        mem: &mut Memory<'_>,
        kind: i32,
        ptr: i32,
        cap: i32,
    ) -> HostResult<i32> {
        if !(0..=8).contains(&kind) {
            return self.stop(Status::Trap, Vec::new());
        }
        // The remaining budget is reported as it stood before this getter's
        // own copy debit.
        let remaining = (kind == 8).then(|| self.runtime.machine.borrow().meter.remaining());
        let len = match kind {
            3 => 33,
            4 | 5 => 8,
            7 => self.runtime.machine.borrow().context.producer.len(),
            _ => 32,
        };
        // The span is validated and the copy debited before a byte moves, and
        // the bytes are copied straight from the context: a length query or
        // a short buffer moves nothing, whatever the producer's size.
        let Some(start) = self.output_span(mem, ptr, cap, len)? else {
            return Ok(len as i32);
        };
        let dst = &mut mem.as_deref_mut().expect("validated span")[start..start + len];
        let m = self.runtime.machine.borrow();
        match kind {
            0 => dst.copy_from_slice(&m.context.chain),
            1 => dst.copy_from_slice(&m.transaction),
            2 => dst.copy_from_slice(&self.id),
            3 => {
                dst[0] = u8::from(self.caller.is_some());
                dst[1..].copy_from_slice(&self.caller.unwrap_or([0; 32]));
            }
            4 => dst.copy_from_slice(&m.context.height.to_le_bytes()),
            5 => dst.copy_from_slice(&m.context.timestamp.to_le_bytes()),
            6 => dst.copy_from_slice(&m.context.parent),
            7 => dst.copy_from_slice(&m.context.producer),
            _ => {
                for (i, n) in remaining.expect("read above").0.iter().enumerate() {
                    dst[i * 8..i * 8 + 8].copy_from_slice(&n.to_le_bytes());
                }
            }
        }
        Ok(len as i32)
    }
    pub fn storage_get(
        &mut self,
        mem: &mut Memory<'_>,
        key: i32,
        ptr: i32,
        cap: i32,
    ) -> HostResult<i32> {
        self.span(mem, ptr, cap)?;
        let parts = self.read_parts(mem, &[(key, 32)])?;
        let key = self.key(&parts[0]);
        // The read is charged on the length alone; the value is fetched only
        // when it fits, after its copy is debited.
        let len = match self.runtime.read_len(key) {
            Ok(Some(len)) => len,
            Ok(None) => return Ok(-1),
            Err(status) => return self.stop(status, Vec::new()),
        };
        let Some(start) = self.output_span(mem, ptr, cap, len as usize)? else {
            return Ok(len as i32);
        };
        let value = match self.runtime.record_value(key) {
            Ok(value) => value,
            Err(status) => return self.stop(status, Vec::new()),
        };
        mem.as_deref_mut().expect("validated span")[start..start + value.len()]
            .copy_from_slice(&value);
        Ok(len as i32)
    }
    pub fn code_hash(
        &mut self,
        mem: &mut Memory<'_>,
        target: i32,
        ptr: i32,
        cap: i32,
    ) -> HostResult<i32> {
        self.span(mem, ptr, cap)?;
        let parts = self.read_parts(mem, &[(target, 32)])?;
        let target: Hash = parts[0].as_slice().try_into().unwrap();
        match self.runtime.read_record(manifest_key(target)) {
            Ok(Some(manifest)) => self.output(mem, ptr, cap, &manifest[..32]),
            Ok(None) => Ok(-1),
            Err(status) => self.stop(status, Vec::new()),
        }
    }
    pub fn storage_set(
        &mut self,
        mem: &mut Memory<'_>,
        key: i32,
        value: i32,
        len: i32,
    ) -> HostResult<()> {
        self.writable()?;
        // A getter answers 2^32 - 1 for an absent record, so no value is that
        // long (wiki: vm/host).
        if len as u32 == u32::MAX {
            return self.stop(Status::Trap, Vec::new());
        }
        let mut parts = self.read_parts(mem, &[(key, 32), (value, len)])?;
        let key = self.key(&parts[0]);
        match self.runtime.write_record(key, Some(parts.remove(1))) {
            Ok(()) => Ok(()),
            Err(status) => self.stop(status, Vec::new()),
        }
    }
    pub fn storage_delete(&mut self, mem: &mut Memory<'_>, key: i32) -> HostResult<()> {
        self.writable()?;
        let parts = self.read_parts(mem, &[(key, 32)])?;
        let key = self.key(&parts[0]);
        match self.runtime.write_record(key, None) {
            Ok(()) => Ok(()),
            Err(status) => self.stop(status, Vec::new()),
        }
    }
    /// `return` and `revert`: the result span is validated and debited, then
    /// the invocation ends with the given status.
    pub fn finish(
        &mut self,
        mem: &mut Memory<'_>,
        status: Status,
        ptr: i32,
        len: i32,
    ) -> HostResult<()> {
        let range = self.span(mem, ptr, len)?;
        let n = range.len() as u64;
        self.charge(Resources([tier(mem).bytes(n), 0, n, 0]))?;
        let bytes = mem.as_deref().expect("validated span")[range].to_vec();
        self.stop(status, bytes)
    }
    pub fn emit(&mut self, mem: &mut Memory<'_>, ptr: i32, len: i32) -> HostResult<()> {
        self.writable()?;
        let range = self.span(mem, ptr, len)?;
        let n = range.len() as u64;
        self.charge(Resources([tier(mem).bytes(n), 0, 36 + n, 0]))?;
        let event = Event {
            contract: self.id,
            payload: mem.as_deref().expect("validated span")[range].to_vec(),
        };
        self.runtime.machine.borrow_mut().events.push(event);
        Ok(())
    }
    /// Debits, in a child's frame just opened, as the
    /// child's first debit, its receipt of `n` argument bytes at the caller's
    /// tier. `Some(status)` when the child cannot pay: it ends there, having
    /// consumed nothing, and its frame is closed again.
    fn open_child(&mut self, tier: Tier, n: u64) -> Option<Status> {
        if let Err(status) = self.runtime.charge(Resources::compute(tier.bytes(n))) {
            self.runtime.machine.borrow_mut().meter.pop();
            return Some(status);
        }
        None
    }
    pub fn call(
        &mut self,
        mem: &mut Memory<'_>,
        target: i32,
        ptr: i32,
        len: i32,
        budget: i32,
        flags: i32,
    ) -> HostResult<i32> {
        if flags != 0 && flags != 1 {
            return self.stop(Status::Trap, Vec::new());
        }
        // Every span is checked before any byte is charged or copied; the
        // caller pays for the target and the budget, the child for its input.
        let target = self.span(mem, target, 32)?;
        let input = self.span(mem, ptr, len)?;
        let budget = self.span(mem, budget, 32)?;
        let tier = tier(mem);
        self.charge(Resources::compute(tier.bytes(64)))?;
        let m = mem
            .as_deref()
            .expect("spans were validated against a memory");
        let target: Hash = m[target].try_into().unwrap();
        let requested = resource_bytes(&m[budget]);
        self.last = Rc::new(Vec::new());
        if !self.runtime.machine.borrow_mut().meter.push(requested) {
            return Ok(128);
        }
        if let Some(status) = self.open_child(tier, input.len() as u64) {
            return Ok(i32::from(status as u8));
        }
        let input = mem.as_deref().expect("validated span")[input].to_vec();
        let readonly = self.readonly || flags == 1;
        self.suspend(Pending::Call {
            target,
            input,
            readonly,
        })
    }
    pub fn deploy(
        &mut self,
        mem: &mut Memory<'_>,
        code: i32,
        code_len: i32,
        ptr: i32,
        len: i32,
        budget: i32,
    ) -> HostResult<i32> {
        self.writable()?;
        // As for `call`: the caller pays for the budget, the deployment for
        // receiving its code and input, before it takes an ordinal.
        let code = self.span(mem, code, code_len)?;
        let input = self.span(mem, ptr, len)?;
        let budget = self.span(mem, budget, 32)?;
        let tier = tier(mem);
        self.charge(Resources::compute(tier.bytes(32)))?;
        let requested = resource_bytes(&mem.as_deref().expect("validated span")[budget]);
        self.last = Rc::new(Vec::new());
        if !self.runtime.machine.borrow_mut().meter.push(requested) {
            return Ok(128);
        }
        let n = (code.len() + input.len()) as u64;
        if let Some(status) = self.open_child(tier, n) {
            return Ok(i32::from(status as u8));
        }
        let m = mem.as_deref().expect("validated span");
        let (code, input) = (m[code].to_vec(), m[input].to_vec());
        self.suspend(Pending::Deploy { code, input })
    }
}
