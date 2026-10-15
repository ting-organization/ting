//! Runs a case: installs its programs, executes the steps on the reference VM
//! and compares every observable with what the case declares. A step that
//! leaves the state unchanged is run again warm and again after the code
//! cache is cleared, and must produce the identical receipt. Under the
//! `oracle` feature the whole case runs on the oracle engine as well, and the
//! receipts of the two engines must be identical byte for byte.
#![allow(dead_code)]
use crate::{dsl, shape};
use std::{collections::BTreeMap, rc::Rc};
use ting_core::{
    hash::{Hash, hash, hex},
    meter::Resources,
    protocol::{Encode, Event, Genesis, Payload, Receipt, Status, Transaction},
    state::{Entry, Position, State, StateReader, code_key, manifest_key, record_key},
    vm::{Context, Runtime, profile},
};
use vm_model::{BUDGET, Ledger};

/// The identifier the case's program is installed under.
pub const TARGET: Hash = [0x11; 32];
/// Further installed programs, for calls between programs.
pub const CHILD: Hash = [0x12; 32];
pub const GRANDCHILD: Hash = [0x13; 32];

/// The block context of every case; the context getters report these values.
pub fn context() -> Context {
    Context {
        chain: Genesis::experimental().chain_id(),
        height: 9,
        timestamp: 1_700_000_060,
        parent: [0x22; 32],
        producer: b"fixture miner".to_vec(),
    }
}
/// A logical key with one distinguishing byte.
pub fn k(n: u8) -> [u8; 32] {
    let mut key = [0; 32];
    key[0] = n;
    key
}
/// The storage key of program `id`'s record under the logical key.
pub fn storage_key(id: Hash, logical: [u8; 32]) -> Hash {
    record_key(2, id, logical)
}
/// The manifest and code records that install `code` as program `id`, the
/// code price as the model reads it from the framing.
pub fn installation(id: Hash, code: &[u8]) -> [(Hash, Vec<u8>); 2] {
    let mut manifest = hash("code", code).to_vec();
    manifest.extend((code.len() as u32).to_le_bytes());
    manifest.extend(vm_model::code_price(code).to_le_bytes());
    [(manifest_key(id), manifest), (code_key(id), code.to_vec())]
}
/// A call of `target` with `input` under the standard budget, as the harness
/// encodes it.
pub fn call_transaction(target: Hash, input: &[u8]) -> Transaction {
    Transaction {
        budget: Resources(BUDGET),
        payload: Payload::Call {
            target,
            input: input.to_vec(),
        },
    }
}
/// The identifier of the `ordinal`-th deployment made by `tx` (wiki:
/// data/records): the chain, the transaction and the ordinal, hashed.
pub fn identifier(tx: &Transaction, ordinal: u64) -> Hash {
    let chain = context().chain;
    let mut bytes = chain.to_vec();
    bytes.extend(tx.id(chain));
    bytes.extend(ordinal.to_le_bytes());
    hash("contract", &bytes)
}
/// What entering the program in `text` costs, for ledgers of nested calls:
/// the first invocation of its code in the transaction.
pub fn entry_of(text: &str) -> u64 {
    shape::shape(&dsl::wasm(text)).entry().compute()
}
/// The same for code the transaction has already prepared.
pub fn warm_entry_of(text: &str) -> u64 {
    shape::shape(&dsl::wasm(text)).entry().warm().compute()
}
/// The code price and CodeHash of a program that is deployed from inside, for
/// a ledger's `validation`.
pub fn validation_of(text: &str) -> u64 {
    let code = dsl::wasm(text);
    vm_model::code_price(&code).saturating_add(vm_model::code_hash(code.len() as u64))
}

/// A program's complete storage: logical keys and values.
type Records = Vec<([u8; 32], Vec<u8>)>;

pub struct Case {
    text: String,
    code: Vec<u8>,
    name: Option<String>,
    /// Programs installed before the first step.
    programs: Vec<(Hash, Vec<u8>)>,
    /// Storage present before the first step: program, logical key, value.
    storage: Vec<(Hash, [u8; 32], Vec<u8>)>,
    /// Producer data of the block, when a case needs other than the default.
    producer: Option<Vec<u8>>,
    steps: Vec<Step>,
}
struct Step {
    tx: Transaction,
    status: Status,
    result: Vec<u8>,
    events: Vec<Event>,
    /// The complete storage of a program after the step.
    storage: Vec<(Hash, Records)>,
    /// Programs the step deploys from inside: ordinal and code.
    installs: Vec<(u64, Vec<u8>)>,
    ledger: Option<Ledger>,
    /// Exact counters, for the few failures before the program is entered.
    consumed: Option<[u64; 4]>,
}

/// The bytes of a program that is installed without being deployed: it must
/// be one the profile accepts, or the case is wrong, not the VM.
fn installed(text: &str) -> Vec<u8> {
    let code = dsl::wasm(text);
    if let Err(e) = profile::validate(&code) {
        panic!(
            "an installed program must validate, this one does not ({}):\n{text}",
            e.0
        );
    }
    code
}

/// A case for the program given in WebAssembly text, installed as `TARGET`.
pub fn case(text: &str) -> Case {
    let code = installed(text);
    Case {
        programs: vec![(TARGET, code.clone())],
        code,
        text: text.to_string(),
        name: None,
        storage: Vec::new(),
        producer: None,
        steps: Vec::new(),
    }
}
/// A case that starts from an empty state and deploys its program itself.
pub fn deployment(text: &str) -> Case {
    Case {
        code: dsl::wasm(text),
        text: text.to_string(),
        name: None,
        programs: Vec::new(),
        storage: Vec::new(),
        producer: None,
        steps: Vec::new(),
    }
}
/// The same for a module given as bytes, for encodings no text can express.
pub fn deployment_bytes(code: &[u8]) -> Case {
    Case {
        code: code.to_vec(),
        text: format!("{} bytes: {}", code.len(), hex(code)),
        name: None,
        programs: Vec::new(),
        storage: Vec::new(),
        producer: None,
        steps: Vec::new(),
    }
}
impl Case {
    /// A name for the case's messages; the program text is shown either way.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    /// Another program installed before the first step.
    pub fn with_program(mut self, id: Hash, text: &str) -> Self {
        self.programs.push((id, installed(text)));
        self
    }
    /// Storage of program `id` present before the first step.
    pub fn with_storage<V: AsRef<[u8]>>(
        mut self,
        id: Hash,
        records: impl IntoIterator<Item = ([u8; 32], V)>,
    ) -> Self {
        for (logical, value) in records {
            self.storage.push((id, logical, value.as_ref().to_vec()));
        }
        self
    }
    /// The block's producer data, instead of the default.
    pub fn with_producer(mut self, producer: impl AsRef<[u8]>) -> Self {
        self.producer = Some(producer.as_ref().to_vec());
        self
    }
    fn context(&self) -> Context {
        let mut context = context();
        if let Some(producer) = &self.producer {
            context.producer = producer.clone();
        }
        context
    }
    fn step(mut self, payload: Payload) -> Self {
        self.steps.push(Step {
            tx: Transaction {
                budget: Resources(BUDGET),
                payload,
            },
            status: Status::Success,
            result: Vec::new(),
            events: Vec::new(),
            storage: Vec::new(),
            installs: Vec::new(),
            ledger: None,
            consumed: None,
        });
        self
    }
    fn last(&mut self) -> &mut Step {
        self.steps
            .last_mut()
            .expect("a step comes before its expectations")
    }
    /// A call of the program with no input, under the standard budget.
    pub fn call(self) -> Self {
        self.call_with(&[])
    }
    /// A call of the program with `input`, under the standard budget.
    pub fn call_with(self, input: &[u8]) -> Self {
        self.step(Payload::Call {
            target: TARGET,
            input: input.to_vec(),
        })
    }
    /// A deployment of the case's program with no input.
    pub fn deploy(self) -> Self {
        self.deploy_with(&[])
    }
    /// A deployment of the case's program with `input`. On success the result
    /// is the identifier and the program's records are expected under it.
    pub fn deploy_with(self, input: &[u8]) -> Self {
        let code = self.code.clone();
        self.step(Payload::Deploy {
            code,
            input: input.to_vec(),
        })
    }
    /// A call of the program the last deployment step deployed.
    pub fn then_call(self) -> Self {
        self.then_call_with(&[])
    }
    pub fn then_call_with(self, input: &[u8]) -> Self {
        let deploy = self
            .steps
            .iter()
            .rev()
            .find(|s| matches!(s.tx.payload, Payload::Deploy { .. }))
            .expect("a deployment step comes before the call of its program");
        let target = identifier(&deploy.tx, 0);
        self.step(Payload::Call {
            target,
            input: input.to_vec(),
        })
    }
    /// The step's budget, C/S/D/G.
    pub fn budget(mut self, budget: [u64; 4]) -> Self {
        self.last().tx.budget = Resources(budget);
        self
    }
    /// The step's status; success unless stated.
    pub fn expect(mut self, status: Status) -> Self {
        self.last().status = status;
        self
    }
    /// The step's result bytes.
    pub fn result(mut self, bytes: impl AsRef<[u8]>) -> Self {
        self.last().result = bytes.as_ref().to_vec();
        self
    }
    /// The payloads of the events the program emits, in order.
    pub fn events<P: AsRef<[u8]>>(self, payloads: impl IntoIterator<Item = P>) -> Self {
        self.events_from(TARGET, payloads)
    }
    /// Events emitted by program `id`, appended in order.
    pub fn events_from<P: AsRef<[u8]>>(
        mut self,
        id: Hash,
        payloads: impl IntoIterator<Item = P>,
    ) -> Self {
        self.last()
            .events
            .extend(payloads.into_iter().map(|p| Event {
                contract: id,
                payload: p.as_ref().to_vec(),
            }));
        self
    }
    /// The program's complete storage after the step, by logical key; none
    /// unless stated.
    pub fn records<V: AsRef<[u8]>>(self, records: impl IntoIterator<Item = ([u8; 32], V)>) -> Self {
        self.records_of(TARGET, records)
    }
    /// The program has no storage after the step, whatever it started with.
    pub fn no_records(self) -> Self {
        self.records_of(TARGET, Vec::<([u8; 32], Vec<u8>)>::new())
    }
    /// The complete storage of program `id` after the step.
    pub fn records_of<V: AsRef<[u8]>>(
        mut self,
        id: Hash,
        records: impl IntoIterator<Item = ([u8; 32], V)>,
    ) -> Self {
        let records = records
            .into_iter()
            .map(|(logical, value)| (logical, value.as_ref().to_vec()))
            .collect();
        self.last().storage.push((id, records));
        self
    }
    /// The step deploys `text` from inside as its `ordinal`-th deployment; on
    /// success the program's records are expected under the derived identifier.
    pub fn installs(mut self, ordinal: u64, text: &str) -> Self {
        let code = dsl::wasm(text);
        self.last().installs.push((ordinal, code));
        self
    }
    /// What the program does on this step; the model turns it into the exact
    /// counters the receipt must show.
    pub fn charged(mut self, ledger: Ledger) -> Self {
        self.last().ledger = Some(ledger);
        self
    }
    /// The exact counters, C/S/D/G, for a step that fails before the program
    /// is entered and so has no ledger.
    pub fn consumed(mut self, counters: [u64; 4]) -> Self {
        self.last().consumed = Some(counters);
        self
    }
    fn label(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => self
                .text
                .lines()
                .last()
                .unwrap_or("case")
                .trim()
                .to_string(),
        }
    }
    pub fn run(self) {
        let (_, own) = execute(&self, Runtime::new);
        #[cfg(feature = "oracle")]
        {
            let (_, oracle) = execute(&self, Runtime::with_oracle);
            assert_eq!(
                own,
                oracle,
                "{}: the oracle engine's receipts differ\n{}",
                self.label(),
                self.text
            );
        }
        let _ = own;
    }
    /// The records the case expects to find before its first step, for a
    /// store that is to hold them.
    pub fn initial_records(&self) -> BTreeMap<Hash, Vec<u8>> {
        Expected::initial(self).map()
    }
    /// Runs the case on the reference VM over `state`, which holds the
    /// initial records already, and returns the runtime with its changes.
    pub fn run_on(self, state: State) -> Runtime {
        execute_on(&self, state, Runtime::new).0
    }
}

/// The logical state a case expects: installed programs and their storage,
/// kept by program and logical key so that a declaration can replace one
/// program's storage as a whole.
#[derive(Clone)]
struct Expected {
    records: BTreeMap<Hash, Vec<u8>>,
    storage: BTreeMap<(Hash, [u8; 32]), Vec<u8>>,
}
impl Expected {
    fn initial(case: &Case) -> Self {
        let mut records = BTreeMap::new();
        for (id, code) in &case.programs {
            records.extend(installation(*id, code));
        }
        let storage = case
            .storage
            .iter()
            .map(|(id, logical, value)| ((*id, *logical), value.clone()))
            .collect();
        Self { records, storage }
    }
    fn map(&self) -> BTreeMap<Hash, Vec<u8>> {
        let mut map = self.records.clone();
        for ((id, logical), value) in &self.storage {
            map.insert(storage_key(*id, *logical), value.clone());
        }
        map
    }
}

/// The initial records behind a reader, as a node's store would hold them, so
/// that every change the VM makes, deletions included, is reported against them.
struct MemoryReader {
    records: BTreeMap<Hash, Vec<u8>>,
    nodes: BTreeMap<Position, Entry>,
}
impl StateReader for MemoryReader {
    fn record(&self, key: &Hash) -> Result<Option<Vec<u8>>, String> {
        Ok(self.records.get(key).cloned())
    }
    fn node(&self, position: &Position) -> Result<Option<Entry>, String> {
        Ok(self.nodes.get(position).copied())
    }
}
fn backed(initial: &BTreeMap<Hash, Vec<u8>>) -> State {
    let mut seed = State::default();
    for (key, value) in initial {
        seed.set(*key, Some(value.clone()));
    }
    seed.commit();
    let nodes = seed
        .changes()
        .unwrap()
        .nodes
        .into_iter()
        .filter_map(|(position, node)| node.map(|node| (position, node)))
        .collect();
    let reader = MemoryReader {
        records: initial.clone(),
        nodes,
    };
    State::backed(Rc::new(reader), seed.len(), seed.root())
}

/// Runs every step on a runtime built by `make` over the initial records
/// behind a reader, and returns the runtime and the encoded receipts.
fn execute(case: &Case, make: fn(State, Context) -> Runtime) -> (Runtime, Vec<String>) {
    let initial = Expected::initial(case).map();
    execute_on(case, backed(&initial), make)
}
/// The same over a given state that holds the initial records already.
fn execute_on(
    case: &Case,
    state: State,
    make: fn(State, Context) -> Runtime,
) -> (Runtime, Vec<String>) {
    let mut expected = Expected::initial(case);
    let initial = expected.map();
    let runtime = make(state, case.context());
    let mut receipts = Vec::new();
    for (index, step) in case.steps.iter().enumerate() {
        let label = format!("{} step {index}", case.label());
        let before = runtime.state_root();
        let receipt = runtime
            .execute(&step.tx)
            .unwrap_or_else(|e| panic!("{label}: {e}\n{}", case.text));
        check(
            &label,
            case,
            &runtime,
            &initial,
            &mut expected,
            step,
            &receipt,
        );
        let unchanged = runtime.state_root() == before;
        runtime.commit();
        let encoded = hex(&receipt.encode());
        if unchanged {
            // Warm code cache, then a cleared one: the receipt may not depend
            // on either.
            for (pass, clear) in [("warm cache", false), ("cleared cache", true)] {
                if clear {
                    runtime.clear_cache();
                }
                let again = runtime.execute(&step.tx).unwrap();
                assert_eq!(
                    hex(&again.encode()),
                    encoded,
                    "{label}: receipt changed on the {pass}\n{}",
                    case.text
                );
                runtime.commit();
            }
        }
        receipts.push(encoded);
    }
    (runtime, receipts)
}

fn check(
    label: &str,
    case: &Case,
    runtime: &Runtime,
    initial: &BTreeMap<Hash, Vec<u8>>,
    expected: &mut Expected,
    step: &Step,
    receipt: &Receipt,
) {
    let text = &case.text;
    let deployed = match &step.tx.payload {
        Payload::Deploy { code, .. } => Some(code),
        Payload::Call { .. } => None,
    };
    let succeeded = step.status == Status::Success;
    assert_eq!(receipt.status, step.status, "{label}: status\n{text}");
    // A successful top-level deployment returns the identifier and nothing else.
    let result = match deployed {
        Some(_) if succeeded => identifier(&step.tx, 0).to_vec(),
        _ => step.result.clone(),
    };
    assert_eq!(
        hex(&receipt.result),
        hex(&result),
        "{label}: result\n{text}"
    );
    let shown = |events: &[Event]| -> Vec<(String, String)> {
        events
            .iter()
            .map(|e| (hex(&e.contract), hex(&e.payload)))
            .collect()
    };
    assert_eq!(
        shown(&receipt.events),
        shown(&step.events),
        "{label}: events\n{text}"
    );
    // The complete logical state after the step: what was installed, changed
    // by what the VM reports, must be the state expected after the previous
    // step with each declared program's storage replaced, plus what the step
    // deployed. A step that declares nothing changes nothing.
    let mut after: BTreeMap<Hash, Vec<u8>> = initial.clone();
    for change in runtime.state_changes().unwrap().records {
        match change.after {
            Some(value) => after.insert(change.key, value),
            None => after.remove(&change.key),
        };
    }
    for (id, records) in &step.storage {
        expected.storage.retain(|(program, _), _| program != id);
        for (logical, value) in records {
            expected.storage.insert((*id, *logical), value.clone());
        }
    }
    if succeeded {
        if let Some(code) = deployed {
            expected
                .records
                .extend(installation(identifier(&step.tx, 0), code));
        }
        for (ordinal, code) in &step.installs {
            expected
                .records
                .extend(installation(identifier(&step.tx, *ordinal), code));
        }
    }
    let shown_records = |records: &BTreeMap<Hash, Vec<u8>>| -> BTreeMap<String, String> {
        records.iter().map(|(k, v)| (hex(k), hex(v))).collect()
    };
    assert_eq!(
        shown_records(&after),
        shown_records(&expected.map()),
        "{label}: records after the step\n{text}"
    );
    if let Some(counters) = step.consumed {
        assert_eq!(receipt.used.0, counters, "{label}: C/S/D/G\n{text}");
    }
    if let Some(ledger) = step.ledger {
        // A result is debited when it is returned or reverted; events only
        // survive success.
        let result = match step.status {
            Status::Success | Status::Revert => step.result.len() as u64,
            _ => 0,
        };
        let events: Vec<u64> = if succeeded {
            step.events.iter().map(|e| e.payload.len() as u64).collect()
        } else {
            Vec::new()
        };
        let counters = match &step.tx.payload {
            Payload::Call { input, .. } => {
                let entry = shape::shape(&case.code).entry();
                vm_model::call(input.len() as u64, entry, ledger, result, &events)
            }
            Payload::Deploy { code, input } => {
                let entry = shape::shape(code).entry();
                vm_model::deploy(code, input.len() as u64, entry, ledger, &events)
            }
        };
        assert_eq!(
            receipt.used.0, counters,
            "{label}: C/S/D/G for {ledger:?}\n{text}"
        );
    }
}

/// The harness must fail a case that misstates any observable; otherwise a
/// green run proves nothing.
mod self_test {
    use super::{case, k};
    use crate::dsl::{import, invoke, memory, module};
    use ting_core::protocol::Status;
    use vm_model::{Ledger, cost};

    const BODY: &str = "i32.const 0 i64.const 42 i64.store i32.const 0 i32.const 8 call $return";
    fn returning_forty_two() -> String {
        module(&[&import("return"), &memory(1, 1), &invoke(BODY)])
    }
    #[test]
    fn a_correct_case_passes() {
        case(&returning_forty_two())
            .call()
            .result(42u64.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(BODY),
                ..Default::default()
            })
            .run();
    }
    #[test]
    #[should_panic(expected = "status")]
    fn a_wrong_status_fails() {
        case(&returning_forty_two())
            .call()
            .expect(Status::Trap)
            .run();
    }
    #[test]
    #[should_panic(expected = "result")]
    fn a_wrong_result_fails() {
        case(&returning_forty_two())
            .call()
            .result(41u64.to_le_bytes())
            .run();
    }
    #[test]
    #[should_panic(expected = "C/S/D/G")]
    fn a_wrong_ledger_fails() {
        case(&returning_forty_two())
            .call()
            .result(42u64.to_le_bytes())
            .charged(Ledger {
                dispatch: cost(BODY) + 1,
                ..Default::default()
            })
            .run();
    }
    #[test]
    #[should_panic(expected = "records after the step")]
    fn an_undeclared_record_fails() {
        let writer = module(&[
            &import("storage_set"),
            &memory(1, 1),
            &invoke("i32.const 0 i32.const 0 i32.const 8 call $storage_set"),
        ]);
        case(&writer).call().run();
    }
    #[test]
    #[should_panic(expected = "records after the step")]
    fn a_declared_record_that_is_not_written_fails() {
        case(&returning_forty_two())
            .call()
            .result(42u64.to_le_bytes())
            .records([(k(0), [1u8])])
            .run();
    }
}
