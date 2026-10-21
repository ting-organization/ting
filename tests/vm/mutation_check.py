"""Check curated consensus faults against exact semantic witnesses.

Requires Python 3.9+, Git and the pinned Rust toolchain. Run from any directory.
Only a temporary source copy is mutated; the checkout and seed stay untouched.
This is a small sensitivity audit, not a claim of exhaustive mutation coverage.
"""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[2]
# name, source file, exact original, replacement, occurrences, test
MUTATIONS = [
    (
        "omit-span-price", "src/vm/price.rs",
        "self.word * words(n) + self.span * spans",
        "self.word * words(n) + self.span * spans * 0", 1,
        "host::input_and_producer_getters_copy_whole_or_nothing_across_price_steps",
    ),
    (
        "round-byte-words-down", "src/vm/profile.rs",
        "bytes.div_ceil(WORD)", "bytes / WORD", 1,
        "budgets::output_debits_are_atomic_at_every_memory_step_and_word_edge",
    ),
    (
        "keep-pre-growth-prices", "src/vm/interp.rs",
        "self.tier = Tier::of(mem.len() as u64);", "// deliberately retain the old tier", 1,
        "calls::grown_parents_resume_at_their_own_prices_after_a_child",
    ),
    (
        "skip-large-memory-copy", "src/vm/interp.rs",
        "mem.copy_within(s..s + n, d);",
        "if mem.len() < 64 * 1024 * 1024 { mem.copy_within(s..s + n, d); }", 1,
        "heavy::random_small_copies_in_a_large_memory_fill_the_compute",
    ),
    (
        "retain-refused-child-result", "src/vm/host.rs",
        "self.last = Rc::new(Vec::new());", "// deliberately retain the last child's bytes", 2,
        "calls::the_last_result_is_cleared_by_a_refused_call",
    ),
    (
        "reverse-state-node-children", "src/state.rs",
        'hash_parts("state-node", &[&depth.to_le_bytes(), &left, &right])',
        'hash_parts("state-node", &[&depth.to_le_bytes(), &right, &left])', 1,
        "seed::a_case_stating_its_block_values_is_checked_in_its_block",
    ),
    (
        "rewind-ordinal-on-rollback", "src/vm/runtime.rs",
        "fn rollback(&self, checkpoint: usize, events: usize) {\n        let mut m = self.machine.borrow_mut();",
        "fn rollback(&self, checkpoint: usize, events: usize) {\n        let mut m = self.machine.borrow_mut();\n        m.ordinal = 0;", 1,
        "calls::failed_initializers_undo_descendants_without_rewinding_ordinals_or_preparation",
    ),
    (
        "rollback-an-earlier-sibling-event", "src/vm/runtime.rs",
        "m.events.truncate(events);", "m.events.truncate(events.saturating_sub(1));", 1,
        "isolation::sibling_and_grandchild_effects_rollback_to_their_own_checkpoints",
    ),
    (
        "allow-a-new-native-host-name", "src/vm/decode.rs",
        'b"input" => Self::Input,', 'b"input" | b"sha256" => Self::Input,', 1,
        "validation::consensus_extensions_are_rejected_even_when_the_program_never_uses_them",
    ),
]


def run(cwd, test, log, env):
    command = ["cargo", "test", "--release", "--locked", "--test", "vm", test,
               "--", "--exact"]
    started = time.monotonic()
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True,
                            text=True, encoding="utf-8", errors="replace", timeout=300)
    output = result.stdout + result.stderr
    log.write_text(output, encoding="utf-8")
    return result.returncode, output, round(time.monotonic() - started, 2)


def main():
    artifacts = ROOT / "target/tmp/seed/mutations"
    artifacts.mkdir(parents=True, exist_ok=True)
    report = {"scope": "Curated faults; a compile error is not a detected mutation.",
              "mutations": []}
    paths = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT
    ).decode("utf-8").split("\0")
    env = os.environ.copy()
    # Keep the mutation build and its artifacts separate from normal builds.
    build_target = (ROOT / "target/mutation-check-build").resolve()
    assert build_target.is_relative_to((ROOT / "target").resolve())
    env["CARGO_TARGET_DIR"] = str(build_target)
    with tempfile.TemporaryDirectory(prefix="mutation-source-", dir=artifacts) as temporary:
        copy = Path(temporary).resolve()
        assert copy.is_relative_to(artifacts.resolve())
        for relative in paths:
            if not relative:
                continue
            source = ROOT / relative
            destination = copy / relative
            assert source.resolve().is_relative_to(ROOT)
            assert destination.resolve().is_relative_to(copy)
            if source.is_file():
                destination.parent.mkdir(parents=True, exist_ok=True)
                # Fresh source mtimes matter: preserving older checkout times
                # can let Cargo reuse the last run's compiled mutant.
                shutil.copyfile(source, destination)

        # Remove only these packages' disposable build products. Dependencies
        # stay cached; no previous compiled mutant may supply a clean baseline.
        subprocess.run(["cargo", "clean", "--release", "--package", "ting-core",
                        "--package", "vm-model"], cwd=copy, env=env, check=True,
                       capture_output=True)

        # Every witness must run exactly once and pass before any mutation.
        for name, _, _, _, _, test in MUTATIONS:
            code, output, _ = run(copy, test, artifacts / f"{name}-baseline.log", env)
            if code or "test result: ok. 1 passed; 0 failed;" not in output:
                raise RuntimeError(f"baseline failed or did not run exactly once: {name}")
        for name, relative, original, replacement, occurrences, test in MUTATIONS:
            source = copy / relative
            pristine = source.read_text(encoding="utf-8")
            if pristine.count(original) != occurrences:
                raise RuntimeError(f"mutation anchor drifted: {name}")
            print(f"Checking {name} ...", flush=True)
            try:
                source.write_text(pristine.replace(original, replacement), encoding="utf-8")
                code, output, seconds = run(copy, test, artifacts / f"{name}.log", env)
                killed = code != 0 and "test result: FAILED. 0 passed; 1 failed;" in output
                report["mutations"].append({"name": name, "source": relative,
                    "witness": test, "detected": killed, "seconds": seconds})
                (artifacts / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
                if not killed:
                    raise RuntimeError(f"surviving mutation or build/runtime infrastructure failure: {name}")
            finally:
                source.write_text(pristine, encoding="utf-8")
    print(f"All {len(MUTATIONS)} mutations detected. Report: {artifacts / 'report.json'}")


if __name__ == "__main__":
    main()
