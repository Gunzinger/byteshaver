# WS7 — Headless Core / Job API (pre-GUI amendment to WS0)

**Depends on:** WS0 merged (all of WS1–WS6 build on it, unchanged).
**Required by:** WS8 (GUI). Also delivers the long-planned "Output logs (JSON)" roadmap item.
**Scope:** make the conversion core drivable by *any* front-end (CLI, GUI, tests, future
web UI) by removing stdout/indicatif/ctrlc/process-exit coupling from the pipeline and
exposing an event-driven job API. No behavior change for CLI users.

---

## 1. Conflict review — what breaks if a GUI sits on today's (and WS0's) core

Audited against current code and WS0 as written:

| # | Conflict | Evidence | Severity for GUI |
|---|----------|----------|------------------|
| C1 | **Reporting is hardwired to stdout + indicatif.** ~25 `println!` sites and the progress bar are driven *inside* the rayon map closure (`pb.inc()`, `pb.set_message(...)`, src/converter/mod.rs:224-270; summary block :276-312). A GUI cannot consume this. | src/converter/mod.rs:35,76,136-312 | 🔴 blocking |
| C2 | **Global `ctrlc::set_handler` inside `run()`.** A GUI must not install process signal handlers from a library call; cancelation must be an injectable flag. | src/converter/mod.rs:165-176 | 🔴 blocking |
| C3 | **`std::process::exit(1)` in the output-dir error path** would kill the GUI process from inside a library call (pre-existing bug WS0 does not yet fix). | src/converter/mod.rs:146-149 | 🔴 blocking |
| C4 | **Input selection is pattern-only** (`ConversionConfig.pattern: String`, cli.rs:20). Drag-and-drop delivers `Vec<PathBuf>` (files *and* folders), not globs. | WS0 §2.8 | 🔴 blocking |
| C5 | **No structured results.** `run()` returns `Result<(), Error>` and only *prints* the summary; a GUI needs a machine-readable `RunReport`. | WS0 §2.4 Outcome exists, but no aggregate struct | 🟠 needed |
| C6 | **Option/config types not serializable.** GUI settings persistence (and JSON logs) need `Serialize/Deserialize` on `ConversionConfig`, `EncoderConfig` and all `*Options` structs; WS0 deliberately kept them plain. | WS0 §2.3 | 🟠 needed |
| C7 | **Thread budget assumes one batch at a time** (`ThreadBudget::global()`). A GUI queue that runs jobs sequentially or allows one job at a time is fine — but the budget must live in a session object, not a global. | WS0 §2.6 | 🟡 design note |
| C8 | **Capability discovery.** Stub modes from WS1/WS2/WS6 (`Error::FeatureDisabled`) are discovered only at runtime per-file; a GUI must gray out impossible encoders up front. | WS1 §6, WS2 §4, WS6 §2 | 🟡 needed by WS8 |
| C9 | `encoder_info()`/`describe()` strings are display-only; GUI wants typed capability info (animated? metadata? bit depths?). | WS0 §2.3 | 🟡 nice-to-have |
| C10 | **Not conflicts:** WS1–WS6 consume only WS0 contracts (`EncoderConfig`, `SourceImage`, `Outcome`, registry) — none touch reporting/signal handling → no rework needed. WS3's passthrough file read, WS4's pipeline-stage orientation bake-in, WS5's memory guard all work unchanged in a GUI session. | — | ✅ |

**Verdict:** no plan needs rework *except WS0*, which needs a small additive follow-up
(this plan). All changes are backward compatible; the CLI becomes the first consumer of
the new API.

## 2. Design

### 2.1 Job API (`src/job/`)

```rust
pub enum InputSelection {
    Pattern(String),              // current CLI behavior (glob)
    Files(Vec<PathBuf>),          // GUI drag-and-drop / file picker
                                  // directories are expanded recursively by the core
}

pub struct JobSpec {
    pub inputs: InputSelection,
    pub output: Option<PathBuf>,
    pub encoder: EncoderConfig,          // WS0; Serialize + Deserialize (C6)
    pub common: ConversionConfig,        // WS0 + exif policy (WS4) + collision policy
}

pub struct JobHandle { /* opaque */ }

impl JobHandle {
    pub fn start(spec: JobSpec, sink: Box<dyn Reporter>, stop: StopFlag,
                 session: &Session) -> JobHandle;   // spawns worker thread(s)
    pub fn join(self) -> RunReport;                 // blocks; CLI uses this
}

pub struct Session {           // one per process/GUI window; owns thread budget (C7)
    pub thread_budget: ThreadBudget,
    pub capabilities: Capabilities,
}

pub struct StopFlag(Arc<AtomicBool>);      // CLI: ctrlc sets it; GUI: Cancel button
impl StopFlag {
    pub fn raised(&self) -> bool;
    pub fn raise(&self);
}
```

### 2.2 Event sink (replaces println/indicatif; resolves C1, C5)

```rust
pub trait Reporter: Send {
    fn on_event(&self, ev: JobEvent);
}

pub enum JobEvent {
    Started { total_files: u64 },
    FileStarted { index: u64, path: PathBuf },
    FileFinished { index: u64, path: PathBuf, outcome: Outcome },  // WS0 Outcome
    Notice { path: Option<PathBuf>, message: String },             // replaces ad-hoc println
    ProgressStats { input_bytes: u64, output_bytes: u64,
                    ok: u64, skipped: u64, errors: u64 },          // replaces pb.set_message
    Finished,
}

pub struct RunReport {             // returned by JobHandle::join / written as JSON log
    pub elapsed: Duration,
    pub files: Vec<(PathBuf, Outcome)>,
    pub totals: Totals,            // sizes, counts, ratios — same math as today's summary
    pub metadata_dropped_count: u64,   // WS4 §5
}
```

Two built-in `Reporter`s ship in the core crate: `StdoutReporter` (the current
indicatif/println output, byte-for-byte compatible) and `JsonlReporter`
(`.byteshaver-log.jsonl`, the roadmap "Output logs" item). The GUI provides its own.

### 2.3 Capability discovery (resolves C8, C9)

```rust
pub struct Capabilities {
    pub encoders: Vec<EncoderInfo>,
}
pub struct EncoderInfo {
    pub config: EncoderConfig,            // default-constructed
    pub name: &'static str,
    pub extension: &'static str,
    pub supports_animation: bool,
    pub supports_exif_embed: bool,        // drives WS4 matrix in the GUI
    pub supports_input_formats: &'static [ImageFormat],
    pub enabled: bool,                    // false when compiled out (WS1/WS2/WS6 stubs)
    pub disabled_reason: Option<&'static str>,
    pub description: String,              // WS0 describe()
}
pub fn capabilities() -> Capabilities;    // registry-driven, feature-aware
```

### 2.4 `run()` re-base

`byteshaver::run(conf, enc)` (WS0 §2.8) becomes a thin wrapper:
`JobSpec::from(...)` → `Session::detect()` → `StopFlag` + ctrlc handler installed by
**the CLI binary** (main.rs), `StdoutReporter` → `JobHandle::join()`. Library callers
never touch signals. Output-dir creation returns `Err` instead of `process::exit(1)`
(C3).

## 3. File-by-file changes

| File | Change |
|------|--------|
| `src/job/mod.rs` (new) | JobSpec/Session/JobHandle/StopFlag/RunReport per §2.1–2.2 |
| `src/job/reporter.rs` (new) | `Reporter` trait, `StdoutReporter` (moves indicatif + all summary println out of `converter/mod.rs`), `JsonlReporter` |
| `src/job/capabilities.rs` (new) | §2.3, fed by the WS0 encoder registry |
| `src/config.rs` | `InputSelection` replaces `pattern: String`; `serde::{Serialize, Deserialize}` derives on all config/option types (C4, C6) |
| `src/pipeline.rs` | takes `&dyn Reporter` + `&StopFlag` + `&Session`; deletes direct `println!`, `pb.*`, ctrlc, `process::exit` |
| `src/main.rs` | installs ctrlc → `StopFlag`, passes `StdoutReporter`; `clean` command keeps its own output path (still allowed to print) |
| `Cargo.toml` | `serde = { version = "1", features = ["derive"] }`, `serde_json` (optional, behind `logs` feature) |
| CLI behavior | unchanged output when using `StdoutReporter`; `--json-log <path>` flag added (cheap win, same PR) |

## 4. Acceptance criteria

- [ ] `byteshaver` binary output byte-identical to WS0 state for all existing commands.
- [ ] `byteshaver::job::JobHandle` can run a conversion with zero stdout output when given a
      null reporter (test asserts captured stdout is empty).
- [ ] Cancel via `StopFlag` mid-queue works without signal handlers (test).
- [ ] All config/option types round-trip through serde (test).
- [ ] `capabilities()` lists every registry encoder; disabled stubs reported with reason.
- [ ] `--json-log` produces valid JSONL; CLI README updated.

## 5. Out of scope

- Any UI code (WS8), async runtime adoption (std threads + channels are sufficient),
  job persistence/resume.
