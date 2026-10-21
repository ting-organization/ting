//! Runs a case: deploys its programs, or installs them for a case that says
//! so, executes the steps on the reference VM and compares every observable
//! with what the case declares. A step that
//! leaves the state unchanged is run again warm and again after the code
//! cache is cleared, and must produce the identical receipt.
#![allow(dead_code)]
use crate::{dsl, gate, shape};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use ting_core::{
    hash::{Hash, hash, hex},
    meter::Resources,
    protocol::{Encode, Event, Payload, Receipt, Status, Transaction},
    state::{Entry, Position, State, StateReader, code_key, manifest_key, record_key},
    vm::{Context, Runtime, profile},
};
use vm_model::{BUDGET, Ledger};

/// The identifier the case's program is installed under. A deployed case's
/// steps and expectations name its programs by these placeholders, which the
/// harness replaces, wherever their bytes appear, with the identifiers the
/// deployments derive.
pub const TARGET: Hash = [0x11; 32];
/// Further programs, for calls between programs.
pub const CHILD: Hash = [0x12; 32];
pub const GRANDCHILD: Hash = [0x13; 32];

/// The block context of every case; the context getters report these values.
/// The chain is the one whose seed the deployed cases make (`network`).
pub fn context() -> Context {
    Context {
        chain: crate::network::genesis().chain_id(),
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
/// The identifier of the `ordinal`-th deployment made by `tx` (rules
/// §5): the chain, the transaction and the ordinal, hashed.
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
/// For a deployed program: everything before its own first instruction, the
/// entry of the gated code and the gate it runs.
pub fn deployed_entry_of(text: &str) -> u64 {
    deployed_shape(text).entry().compute() + vm_model::cost(gate::ON_CALL)
}
/// The same for code the transaction has already prepared.
pub fn deployed_warm_entry_of(text: &str) -> u64 {
    deployed_shape(text).entry().warm().compute() + vm_model::cost(gate::ON_CALL)
}
/// The shape of a deployed program: its code with the gate in front.
pub fn deployed_shape(text: &str) -> shape::Shape {
    shape::shape(&deployed_code(text))
}
/// The code a deployed program is installed with: its own with the gate in
/// front, for a program that starts with no storage and names no other.
pub fn deployed_code(text: &str) -> Vec<u8> {
    gate::gated(&dsl::wasm(text)).expect("a deployed program has a memory")
}
/// The code price and CodeHash of a program that is deployed from inside, for
/// a ledger's `validation`.
pub fn validation_of(text: &str) -> u64 {
    let code = dsl::wasm(text);
    vm_model::code_price(&code).saturating_add(vm_model::code_hash(code.len() as u64))
}

/// A program's complete storage: logical keys and values.
type Records = Vec<([u8; 32], Vec<u8>)>;

/// What only the run knows, for expectations that depend on it: the
/// identifiers the deployments derived, the step's transaction and the block
/// it executes in.
pub struct Facts<'a> {
    ids: &'a [(Hash, Hash)],
    tx: &'a Transaction,
    block: &'a Context,
}
impl Facts<'_> {
    /// The identifier behind a placeholder, or the placeholder itself when
    /// the case deploys no program for it.
    pub fn id(&self, placeholder: Hash) -> Hash {
        real(self.ids, placeholder)
    }
    /// The identifier of the step transaction.
    pub fn tx_id(&self) -> Hash {
        self.tx.id(self.block.chain)
    }
    /// The identifier of the step's `ordinal`-th deployment.
    pub fn deployed(&self, ordinal: u64) -> Hash {
        identifier(self.tx, ordinal)
    }
    /// The block the step executes in.
    pub fn block(&self) -> &Context {
        self.block
    }
}
fn real(ids: &[(Hash, Hash)], placeholder: Hash) -> Hash {
    ids.iter()
        .find(|(p, _)| *p == placeholder)
        .map_or(placeholder, |(_, id)| *id)
}
/// `bytes` with every placeholder of `ids` replaced by its identifier.
fn substituted(bytes: &[u8], ids: &[(Hash, Hash)]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    for (placeholder, id) in ids {
        let mut at = 0;
        while at + 32 <= out.len() {
            if out[at..at + 32] == placeholder[..] {
                out[at..at + 32].copy_from_slice(id);
                at += 32;
            } else {
                at += 1;
            }
        }
    }
    out
}
fn names(bytes: &[u8], placeholder: &Hash) -> bool {
    bytes.windows(32).any(|w| w == placeholder)
}

/// Bytes computed from the run's [`Facts`].
type Derived = Rc<dyn Fn(&Facts) -> Vec<u8>>;
/// A step's result, stated outright or computed from the run.
#[derive(Clone)]
enum Value {
    Bytes(Vec<u8>),
    From(Derived),
}
impl Value {
    fn of(&self, facts: &Facts) -> Vec<u8> {
        match self {
            Value::Bytes(bytes) => bytes.clone(),
            Value::From(f) => f(facts),
        }
    }
}

pub struct Case {
    text: String,
    code: Vec<u8>,
    name: Option<String>,
    /// Whether the case deploys its program, behind the deployment gate, in
    /// a transaction of its own before its steps, as the chain would, instead
    /// of finding it installed under [`TARGET`].
    gated: bool,
    /// Whether the case is one a chain can carry, deployed or a deployment of
    /// its own, and so part of the seed.
    seeded: bool,
    /// Whether an expectation depends on the run, which the seed then checks
    /// in the block the step lands in instead of comparing receipts.
    contextual: bool,
    /// Programs installed before the first step; for a deployed case, the
    /// programs deployed before its own, by placeholder.
    programs: Vec<(Hash, Vec<u8>)>,
    /// Storage present before the first step: program, logical key, value.
    /// A deployed program writes its own in its deployment.
    storage: Vec<(Hash, [u8; 32], Vec<u8>)>,
    /// The placeholders of a deployed case and the identifiers they stand for.
    ids: Vec<(Hash, Hash)>,
    /// Producer data of the block, when a case needs other than the default.
    producer: Option<Vec<u8>>,
    /// Whether the next step starts a block; at the end of the case, whether
    /// the next case does.
    break_next: bool,
    /// Whether each deployment of a deployed case gets a block of its own,
    /// and its steps a later one.
    apart: bool,
    steps: Vec<Step>,
}
struct Step {
    tx: Transaction,
    status: Status,
    result: Value,
    events: Vec<Event>,
    /// The complete storage of a program after the step.
    storage: Vec<(Hash, Records)>,
    /// Programs the step deploys from inside: ordinal and code.
    installs: Vec<(u64, Vec<u8>)>,
    ledger: Option<Ledger>,
    /// Exact counters, for the few failures before the program is entered.
    consumed: Option<[u64; 4]>,
    /// Whether the step starts a block in the seed, after the block that
    /// holds the step before it.
    new_block: bool,
    /// The program a call reaches when it is not the case's own.
    callee: Option<Callee>,
}
/// A program a step calls other than the case's own.
#[derive(Clone)]
enum Callee {
    /// The `ordinal`-th deployment of the case's `step`-th step, with `code`.
    Deployed {
        step: usize,
        ordinal: u64,
        code: Vec<u8>,
    },
    /// An identifier given outright, deployed or not; the step states its
    /// counters with `consumed`.
    Named,
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

fn blank(code: Vec<u8>, text: String, gated: bool, seeded: bool) -> Case {
    Case {
        code,
        text,
        name: None,
        gated,
        seeded,
        contextual: false,
        programs: Vec::new(),
        storage: Vec::new(),
        ids: Vec::new(),
        producer: None,
        break_next: false,
        apart: false,
        steps: Vec::new(),
    }
}
/// A case for the program given in WebAssembly text, installed as `TARGET`.
pub fn case(text: &str) -> Case {
    let code = installed(text);
    Case {
        programs: vec![(TARGET, code.clone())],
        ..blank(code, text.to_string(), false, false)
    }
}
/// A case for the program given in WebAssembly text, deployed behind the
/// deployment gate (`gate`) by a transaction before the case's steps, as the
/// chain would, after the programs it names. Its steps name the program as
/// `TARGET` and the others by their placeholders, which the harness replaces
/// with the identifiers the deployments derive; the deployments' receipts are
/// stated by the harness, and each call is charged the gate besides what the
/// case declares.
pub fn deployed(text: &str) -> Case {
    let code = installed(text);
    if gate::gated(&code).is_none() {
        panic!("a deployed program needs a memory for its gate:\n{text}");
    }
    blank(code, text.to_string(), true, true)
}
/// A case that starts from an empty state and deploys its program itself.
pub fn deployment(text: &str) -> Case {
    blank(dsl::wasm(text), text.to_string(), false, true)
}
/// The same for a module given as bytes, for encodings no text can express.
pub fn deployment_bytes(code: &[u8]) -> Case {
    let text = format!("{} bytes: {}", code.len(), hex(code));
    blank(code.to_vec(), text, false, true)
}
impl Case {
    /// A name for the case's messages; the program text is shown either way.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    /// Another program installed before the first step, or for a deployed
    /// case deployed before its own, with the identifiers of the programs it
    /// names put in its code.
    pub fn with_program(mut self, id: Hash, text: &str) -> Self {
        self.programs.push((id, installed(text)));
        self
    }
    /// Storage of program `id` present before the first step: for a deployed
    /// case, written by the program's deployment.
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
            result: Value::Bytes(Vec::new()),
            events: Vec::new(),
            storage: Vec::new(),
            installs: Vec::new(),
            ledger: None,
            consumed: None,
            new_block: std::mem::take(&mut self.break_next),
            callee: None,
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
    /// A call of `id` with `input`: an identifier the case does not deploy,
    /// or one of its placeholders. The step states its counters with
    /// `consumed`.
    pub fn call_to(mut self, id: Hash, input: &[u8]) -> Self {
        self = self.step(Payload::Call {
            target: id,
            input: input.to_vec(),
        });
        self.last().callee = Some(Callee::Named);
        self
    }
    /// A call with `input` of the program, given as text, that the case's
    /// `step`-th step deployed from inside as its `ordinal`-th deployment.
    pub fn call_deployed(mut self, step: usize, ordinal: u64, text: &str, input: &[u8]) -> Self {
        assert!(step < self.steps.len(), "the deploying step comes first");
        self = self.step(Payload::Call {
            target: [0; 32],
            input: input.to_vec(),
        });
        self.last().callee = Some(Callee::Deployed {
            step,
            ordinal,
            code: dsl::wasm(text),
        });
        self
    }
    /// The next step starts a block in the seed, so that it executes after
    /// everything before it was committed in an earlier block. At the end of
    /// a case, the next case starts a block.
    pub fn next_block(mut self) -> Self {
        self.break_next = true;
        self
    }
    /// Each of a deployed case's deployments in a block of its own, and its
    /// steps in a later one.
    pub fn apart(mut self) -> Self {
        self.apart = true;
        self
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
        self.last().result = Value::Bytes(bytes.as_ref().to_vec());
        self
    }
    /// The step's result as what only the run knows makes it: an identifier
    /// derived from the transaction, or a value of the block. The seed checks
    /// such a step in the block it lands in.
    pub fn result_from(mut self, f: impl Fn(&Facts) -> Vec<u8> + 'static) -> Self {
        self.last().result = Value::From(Rc::new(f));
        self.contextual = true;
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
    pub fn installs(self, ordinal: u64, text: &str) -> Self {
        self.installs_code(ordinal, &dsl::wasm(text))
    }
    /// The same for code given as bytes.
    pub fn installs_code(mut self, ordinal: u64, code: &[u8]) -> Self {
        self.last().installs.push((ordinal, code.to_vec()));
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
        let seeded = self.seeded;
        let case = Rc::new(self.chain_shaped());
        let (_, receipts) = execute(&case, Runtime::new);
        if seeded {
            COLLECTED.with(|collected| {
                if let Some(cases) = collected.borrow_mut().as_mut() {
                    let check = case.contextual.then(|| {
                        let case = case.clone();
                        Rc::new(move |index: usize, block: &Context, receipt: &Receipt| {
                            let label = format!("{} step {index} in its block", case.label());
                            check_receipt(&label, &case, &case.steps[index], receipt, block);
                        }) as Check
                    });
                    cases.push(Collected {
                        label: case.label(),
                        section: String::new(),
                        sources: Vec::new(),
                        transactions: case.steps.iter().map(|s| s.tx.clone()).collect(),
                        receipts,
                        changes: declared_changes(&case),
                        new_block: case.steps.iter().map(|s| s.new_block).collect(),
                        closes: case.break_next,
                        producer: case.producer.clone(),
                        check,
                    });
                }
            });
        }
    }
    /// A deployed case as the chain runs it: the deployment of each program
    /// it names, every program after those whose identifiers its code holds,
    /// each with the receipt the gate's return and the writes of its storage
    /// give it, then its steps, with every placeholder replaced by the
    /// identifier the deployments derived.
    fn chain_shaped(mut self) -> Self {
        if !self.gated {
            resolve_callees(&mut self.steps);
            return self;
        }
        // The gate looks only at the input's length, so its bytes tell cases
        // apart: on one chain each deploys its own programs, with identifiers
        // of their own, even where two cases share a program.
        let mut stated = self.label().into_bytes();
        stated.extend(&self.code);
        for (placeholder, code) in &self.programs {
            stated.extend(placeholder);
            stated.extend(code);
        }
        for (program, logical, value) in &self.storage {
            stated.extend(program);
            stated.extend(logical);
            stated.extend(value);
        }
        for step in &self.steps {
            stated.extend(step.tx.encode());
        }
        let mut pending = std::mem::take(&mut self.programs);
        pending.push((TARGET, self.code.clone()));
        let mut deployments = Vec::new();
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|(_, code)| !pending.iter().any(|(other, _)| names(code, other)))
                .unwrap_or_else(|| {
                    panic!(
                        "{}: programs that name each other, or themselves, cannot be deployed; \
                         pass an identifier in the input or read the own one from the context",
                        self.label()
                    )
                });
            let (placeholder, code) = pending.remove(ready);
            let records: Records = self
                .storage
                .iter()
                .filter(|(program, _, _)| *program == placeholder)
                .map(|(_, logical, value)| (*logical, substituted(value, &self.ids)))
                .collect();
            let gated = gate::gated_with(&substituted(&code, &self.ids), &records)
                .unwrap_or_else(|| panic!("a deployed program needs a memory for its gate"));
            let mut salted = stated.clone();
            salted.extend(placeholder);
            let tx = Transaction {
                budget: Resources(BUDGET),
                payload: Payload::Deploy {
                    code: gated.code.clone(),
                    input: hash("seed-case", &salted)[..gate::MARK].to_vec(),
                },
            };
            let id = identifier(&tx, 0);
            deployments.push(Step {
                tx,
                status: Status::Success,
                result: Value::Bytes(Vec::new()),
                events: Vec::new(),
                storage: if records.is_empty() {
                    Vec::new()
                } else {
                    vec![(id, records)]
                },
                installs: Vec::new(),
                ledger: Some(gated.deployment),
                consumed: None,
                new_block: self.apart,
                callee: None,
            });
            self.ids.push((placeholder, id));
            if placeholder == TARGET {
                self.code = gated.code;
            }
        }
        self.storage.clear();
        let ids = self.ids.clone();
        for step in &mut self.steps {
            if let Payload::Call { target, input } = &mut step.tx.payload {
                assert!(
                    step.callee.is_some() || input.len() != gate::MARK,
                    "a call of a gated program may not take the gate's input"
                );
                *target = real(&ids, *target);
                *input = substituted(input, &ids);
            }
            if let Value::Bytes(bytes) = &mut step.result {
                *bytes = substituted(bytes, &ids);
            }
            for event in &mut step.events {
                event.contract = real(&ids, event.contract);
                event.payload = substituted(&event.payload, &ids);
            }
            for (program, records) in &mut step.storage {
                *program = real(&ids, *program);
                for (_, value) in records {
                    *value = substituted(value, &ids);
                }
            }
            for (_, code) in &mut step.installs {
                *code = substituted(code, &ids);
            }
        }
        resolve_callees(&mut self.steps);
        if self.apart
            && let Some(first) = self.steps.first_mut()
        {
            first.new_block = true;
        }
        deployments.append(&mut self.steps);
        self.steps = deployments;
        self
    }
}

/// The targets of calls of programs that earlier steps deployed from inside,
/// from those steps' transactions as they stand.
fn resolve_callees(steps: &mut [Step]) {
    for i in 0..steps.len() {
        if let Some(Callee::Deployed { step, ordinal, .. }) = steps[i].callee {
            let id = identifier(&steps[step].tx, ordinal);
            if let Payload::Call { target, .. } = &mut steps[i].tx.payload {
                *target = id;
            }
        }
    }
}

/// Checks a step's receipt against the case's expectations in the block it
/// landed in: the step's index, the block and the receipt.
pub type Check = Rc<dyn Fn(usize, &Context, &Receipt)>;
/// A case of the seed as it ran (`seed.rs`): its label and the section the
/// seed puts it in, its transactions in order with their encoded receipts and
/// whether each starts a block, whether the next case starts one, the
/// producer data its blocks must carry, and for a case whose expectations
/// depend on the run, the check of its receipts in the blocks they land in.
pub struct Collected {
    pub label: String,
    pub section: String,
    /// Every source/expanded-variant pair represented by this case, including
    /// aliases when identical fixtures are deliberately placed only once.
    pub sources: Vec<(String, usize)>,
    pub transactions: Vec<Transaction>,
    pub receipts: Vec<String>,
    /// Expected record changes from declarations, never from the runtime.
    pub changes: Vec<Vec<(Hash, Option<Vec<u8>>)>>,
    pub new_block: Vec<bool>,
    pub closes: bool,
    pub producer: Option<Vec<u8>>,
    pub check: Option<Check>,
}
thread_local! {
    static COLLECTED: RefCell<Option<Vec<Collected>>> = const { RefCell::new(None) };
}
/// Runs `cases`, which runs deployed cases, and returns each of them as it
/// ran, every check passed.
pub fn collect(cases: impl FnOnce()) -> Vec<Collected> {
    COLLECTED.with(|c| *c.borrow_mut() = Some(Vec::new()));
    cases();
    COLLECTED.with(|c| c.borrow_mut().take().expect("collecting"))
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
    fn advance(&mut self, step: &Step) {
        for (id, records) in &step.storage {
            self.storage.retain(|(program, _), _| program != id);
            for (logical, value) in records {
                self.storage.insert((*id, *logical), value.clone());
            }
        }
        if step.status == Status::Success {
            if let Payload::Deploy { code, .. } = &step.tx.payload {
                self.records
                    .extend(installation(identifier(&step.tx, 0), code));
            }
            for (ordinal, code) in &step.installs {
                self.records
                    .extend(installation(identifier(&step.tx, *ordinal), code));
            }
        }
    }
}

/// Project a chain-shaped fixture's declarations into per-step changes. The
/// seed applies these to its own plain map before independently hashing it.
fn declared_changes(case: &Case) -> Vec<Vec<(Hash, Option<Vec<u8>>)>> {
    let mut expected = Expected::initial(case);
    let mut before = expected.map();
    assert!(
        before.is_empty(),
        "a seed fixture starts from no installed state"
    );
    case.steps
        .iter()
        .map(|step| {
            expected.advance(step);
            let after = expected.map();
            let mut delta: Vec<_> = before
                .keys()
                .filter(|k| !after.contains_key(*k))
                .map(|k| (*k, None))
                .collect();
            delta.extend(
                after
                    .iter()
                    .filter(|(k, v)| before.get(*k) != Some(*v))
                    .map(|(k, v)| (*k, Some(v.clone()))),
            );
            before = after;
            delta
        })
        .collect()
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
    let block = case.context();
    let result = check_outcome(label, case, step, receipt, &block);
    let text = &case.text;
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
    expected.advance(step);
    let wanted = expected.map();
    let first_difference = after
        .keys()
        .chain(wanted.keys())
        .find(|k| after.get(*k) != wanted.get(*k));
    assert!(
        first_difference.is_none(),
        "{label}: records after the step: {} actual, {} expected; first differing key {:?}\n{text}",
        after.len(),
        wanted.len(),
        first_difference.map(|k| hex(k))
    );
    check_counters(label, case, step, receipt, &result);
}
/// Everything of a step's receipt the case states, in the block it executed in.
fn check_receipt(label: &str, case: &Case, step: &Step, receipt: &Receipt, block: &Context) {
    let result = check_outcome(label, case, step, receipt, block);
    check_counters(label, case, step, receipt, &result);
}
/// The status, the result and the events; the expected result is returned.
fn check_outcome(
    label: &str,
    case: &Case,
    step: &Step,
    receipt: &Receipt,
    block: &Context,
) -> Vec<u8> {
    let text = &case.text;
    let facts = Facts {
        ids: &case.ids,
        tx: &step.tx,
        block,
    };
    assert_eq!(
        receipt.status, step.status,
        "{label}: status
{text}"
    );
    // A successful top-level deployment returns the identifier and nothing else.
    let result = match &step.tx.payload {
        Payload::Deploy { .. } if step.status == Status::Success => {
            identifier(&step.tx, 0).to_vec()
        }
        _ => step.result.of(&facts),
    };
    assert_eq!(
        hex(&receipt.result),
        hex(&result),
        "{label}: result
{text}"
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
        "{label}: events
{text}"
    );
    result
}
/// The counters, stated outright or as the ledger the model turns into them.
fn check_counters(label: &str, case: &Case, step: &Step, receipt: &Receipt, result: &[u8]) {
    let text = &case.text;
    if let Some(counters) = step.consumed {
        assert_eq!(
            receipt.used.0, counters,
            "{label}: C/S/D/G
{text}"
        );
    }
    if let Some(ledger) = step.ledger {
        // A result is debited when it is returned or reverted; events only
        // survive success.
        let result = match step.status {
            Status::Success | Status::Revert => result.len() as u64,
            _ => 0,
        };
        let events: Vec<u64> = if step.status == Status::Success {
            step.events.iter().map(|e| e.payload.len() as u64).collect()
        } else {
            Vec::new()
        };
        let counters = match &step.tx.payload {
            Payload::Call { input, .. } => {
                let (code, gated) = match &step.callee {
                    None => (&case.code, case.gated),
                    Some(Callee::Deployed { code, .. }) => (code, false),
                    Some(Callee::Named) => {
                        panic!("{label}: a call of a named program states its counters")
                    }
                };
                let entry = shape::shape(code).entry();
                // A gated program runs its gate before what the case declares.
                let ledger = if gated {
                    Ledger {
                        dispatch: ledger.dispatch + vm_model::cost(gate::ON_CALL),
                        ..ledger
                    }
                } else {
                    ledger
                };
                vm_model::call(input.len() as u64, entry, ledger, result, &events)
            }
            Payload::Deploy { code, input } => {
                let entry = shape::shape(code).entry();
                vm_model::deploy(code, input.len() as u64, entry, ledger, &events)
            }
        };
        assert_eq!(
            receipt.used.0, counters,
            "{label}: C/S/D/G for {ledger:?}
{text}"
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
