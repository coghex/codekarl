# First runtime epic: design (draft under review)

**Status: approved in design review 3, then amended on 2026-10-18 by the owner's decision that no log line may be dropped (§4.4
"Back-pressure"; the amendment was reviewed with CK-47), and on 2026-10-19 by the owner's decisions on control-path errors (§4.4,
implemented by CK-52) and on milestone 2 (§5, implemented by CK-53 and CK-55 to CK-58).** It includes the owner's 2026-10-18 answers and ck-guide's Hetoimasia reference
evidence (§5.4). This is design; implementation proceeds through the reviewed issues CK-41 onward.

Labels: **Firm** is owner direction; **Recommendation** is ck-manager's proposal for review and owner decision; **Open** is undecided.
The broader game vision (`docs/vision.md`) is context, not the scope of this epic.

## 1. Owner scope

- **Firm.** The first epic is the runtime. Its first observable output is proper logging to stdout. No window or rendering is needed.
- **Firm (2026-10-18 20:23Z).** Milestone 1: "the simple start, load config, initialize, log, then shut down cleanly". The owner chose this
  over also running a small headless simulation loop.
- **Firm (2026-10-18 20:23Z).** Milestone 2: "a lua vm would be the next step after the first milestone"; the same lifecycle as milestone 1,
  "but using the full lua setup instead of just the logger".
- **Firm (2026-10-18 20:28Z). Logging.** The owner accepted ck-guide's 20:24Z proposal ("yes, that is fine"): timestamped, coloured terminal
  output with severity levels and named categories (for example `runtime`, `lua` and `metal`), with configurable verbosity. **Stdout
  only**, with no output file: "that would just be '>'".
- **Firm (2026-10-18 20:28Z). Parallel Lua VMs.** "The important thing i want to be able to do is run parallel vms, so that i can set one
  to the ui, and one to the gameplay. the lua milestone should set up skelentons and print from both of these." Real UI or gameplay is not
  required.
- **Firm (2026-10-18 20:28Z).** Hetoimasia's scripting integration may be inspected as a reference only; it is a different game and is not
  copied.
- **Firm (2026-10-18 20:40Z): "1,2,3 are all correct as recommended".** This answered ck-guide's 20:37Z summary of three recommendations:
  1. separate reusable packages for logging, runtime lifecycle and Lua integration, which the application assembles, with the resource
     manager left until there are actual assets (§3);
  2. `codekarl` starts the runtime directly, and `--config path.toml` selects its configuration (§4.1);
  3. Lua 5.4 through `mlua`, with the interpreter built into the application (§6).
- **Firm (2026-10-18 20:40Z): "i want lua print to be lua print, we should have a seperate log function".** Lua's `print` keeps its standard
  behaviour; logging is a separate function (§5.3).
- **Firm (2026-10-18 20:40Z): "stderr should print to wherever stdout goes".** For example, `codekarl > output.txt` captures both (§4.2).
- **Firm.** Build our own components. Small, well-established Rust libraries are acceptable; nothing as large as a windowing library.
- **Firm (standing project rule).** No window, ECS, renderer abstraction or map representation before its design is discussed with the
  owner. Native windowing is a separate design.
- **Firm.** The packages should be reusable across future Rust/Metal projects.
- **Open (owner asked at 2026-10-17 16:43Z).** "Should we separate logging into a separate package, for example, or should we have a
  resource manager now." Section 3 recommends answers.

## 2. What exists now (accepted)

- **Workspace.** `apps/codekarl` (the `codekarl` binary, the default member) and `crates/gpu-metal` (`ck-gpu-metal`). Edition 2024,
  Rust 1.91, strict workspace lints: `missing_docs`, `unreachable_pub`, `unwrap_used`, Clippy pedantic and documented `unsafe`.
- **CLI contract.** `specs/bootstrap.json`, run by `apps/codekarl/tests/cli_spec.rs`, compares the exact exit code, stdout and stderr for
  each argument list:
  - no arguments prints `Codekarl: Rust engine bootstrap`;
  - `--version` and `--help` print fixed text;
  - unknown or extra arguments exit 1 with `codekarl: unknown arguments; use --help` on stderr;
  - `--metal-smoke` (with the `metal` feature) runs the GPU probe.

  The runner can only pass arguments: it sets no working directory and no extra environment. The spec path is fixed, and
  `CK_REQUIRE_FULL_SPEC=1` turns skipped cases into failures.
- **GPU probe.** `ck-gpu-metal` is a narrow synchronous Metal boundary: `Device::new` and `Device::probe`, neither `Send` nor `Sync`.
  The `gpu_probe` test runs it under Metal validation with a supervised worker process and deadline.
- **Error convention.** `main` maps `Result<(), String>` to exit 1 and prints `codekarl: <message>` on stderr.

## 3. Package boundaries (Firm, owner 2026-10-18 20:40Z; details are Recommendations)

| Package | Path | Purpose | Depends on |
| --- | --- | --- | --- |
| `ck-log` | `crates/log` | Reusable logging: records, levels, categories, filtering, formatting and the stdout sink | std only (see §6) |
| `ck-runtime` | `crates/runtime` | Reusable lifecycle: load configuration, start services in order, stop them in reverse, report failures | `ck-log` |
| `codekarl` | `apps/codekarl` | The game executable: command line, Codekarl's configuration schema and services, probes | `ck-runtime`, `ck-log`, optional `ck-gpu-metal` |
| `ck-lua` (milestone 2) | `crates/lua` | Reusable Lua integration: a VM owner, one Lua state on its own thread, any number per application | `ck-log`, the Lua binding (§6) |

- **Logging is its own package.** Every later package (runtime, Lua, GPU, windowing) needs it, and it is the most reusable piece.
- **The lifecycle is its own small package,** so future Rust/Metal projects get the same start-up and shutdown behaviour. It holds no
  game concepts. Kept minimal: an ordered list of services, each with `start` and `stop`.
- **No resource manager now.** Nothing in either milestone owns resources that need one. Design it when there are real resources and
  ownership requirements (for example GPU buffers or loaded assets).
- **`ck-gpu-metal` is unchanged.** It does not depend on the runtime. A later epic can wrap a `Device` as a runtime service.
- **No simulation loop, window, ECS or resource manager in milestone 1** (owner scope above).

## 4. Milestone 1 behaviour

### 4.1 Command line (Firm, owner 2026-10-18 20:40Z; details are Recommendations)

- `codekarl` with no arguments runs the runtime lifecycle. This replaces the bootstrap line, and its spec case is removed (§8).
- `codekarl --config PATH` runs the lifecycle with that configuration file.
- `--help`, `--version` and `--metal-smoke` keep their current behaviour and do not start the runtime. `--help` text gains `--config`.
- Unknown or extra arguments keep the current error and exit 1.

### 4.2 Lifecycle

1. At executable entry, before parsing arguments, make stderr follow stdout (see "Stderr follows stdout" below).
2. Parse arguments. Usage errors exit 1 before anything else starts. Start the logger with fixed settings (no timestamps, no colour), then
   load the configuration (§4.3). A failure is a plain, deterministic `error` record naming the file and the problem, and exits 1.
3. Apply the configuration's logging section to the logger.
4. Log the start: version, configuration source (file path or built-in defaults) and enabled features.
5. Start each service in order, logging each start. Milestone 1 has no real services beyond logging; the list exists so milestone 2 adds
   Lua as a service without changing the lifecycle.
6. Log that start-up is complete.
7. Stop the started services in reverse order, logging each stop.
8. Log a clean shutdown, flush the log and exit 0.

**Failure and cleanup.**

- If a service fails to start, the runtime logs the error at `error`, stops only the services already started (in reverse order), logs
  the shutdown and exits 1.
- A failure while stopping is logged and does not prevent the remaining services from stopping. The exit status is 1 if any stop
  failed.
- Every log record goes to stdout (Firm, owner: "stdout only"). An error is reported as an `error` record, through the logger, like any
  other record. The earlier proposal of an extra one-line summary on stderr is withdrawn, since stderr now goes where stdout goes.
- **Stderr follows stdout (Firm, owner 20:40Z).** Logging alone cannot cover this, because other code also writes to stderr: Rust's
  default panic message, the Lua C library, Metal's validation layer and any native library.
  - **Recommendation (mechanism, for review):** at executable entry, before argument parsing and before anything is written, the executable
    makes stderr a duplicate of stdout at the file-descriptor level (`dup2(1, 2)`). From then on, everything written to stderr by any code
    reaches stdout's destination: a terminal, a pipe or a file from `>`.
  - This needs one small `unsafe` call, through the `libc` crate (§6), documented like the Metal bridge's. (The incomplete-shutdown exit
    in §4.4 is the executable's only other one.)
  - Bytes written to stderr are not log records. They have no timestamp or category, and their order relative to log records is not
    guaranteed (the logger writes from its own thread). A write to stderr can block on a stalled stdout, just like stdout.
  - **Recommendation:** the panic hook writes through the logger's bounded `log_within` instead of Rust's default stderr message.
- **No runtime-only exception.** The owner's statement has no exception for errors before the runtime starts, so (ck-guide, 20:45Z,
  resolving the narrow question this draft had raised) the routing is set at executable entry and covers every command: usage errors,
  configuration errors, `--help`, `--version` and `--metal-smoke`.
  - A usage error still prints `codekarl: <message>` and exits 1; that text now arrives at stdout's destination.
  - The affected spec expectations change through the code gate (§8). An existing test is not a reason for an exception.
  - **Consequence for `--metal-smoke`:** Metal's validation notices ("Metal API Validation Enabled" and "Metal GPU Validation Enabled"),
    which the spec runner currently filters out of stderr only, will arrive in stdout. The runner's notice filter must also apply to stdout
    (a narrow runner change through the code gate, keeping its exact matching otherwise).
- **Unavoidable limitations (not owner questions):**
  - anything the operating system or dynamic loader writes before `main` runs cannot be routed;
  - if `dup2` itself fails (for example when stdout is not open), stderr stays where it was. The executable then reports the failure on
    whichever stream works and exits with a distinct non-zero status rather than running unrouted.
- **Writing the log must never panic, and shutdown must stay bounded.** `println!` panics when stdout is closed, and any write blocks when
  stdout is a full pipe that nobody reads. Records therefore go through the logger of §4.4. It never drops a line in normal use. An
  ordinary logging thread may wait briefly on lock contention, and indefinitely only when stdout is not being read. Control-path code
  (the lifecycle, supervisor, main thread and panic hook) logs only through the bounded `log_within`, so it
  always reaches its deadlines, cancellation and exit. The final flush has a deadline (§4.4, "Back-pressure"). Exit status still reflects
  the runtime's own result, plus whether the log was fully
  written.
- A panic inside the runtime is reported once through a panic hook as an `error` record. The standard
  unwind then exits non-zero. No service `stop` runs from the hook itself.
- There are no signal handlers in milestone 1. The lifecycle has no long-running wait to interrupt: its own logging is bounded by
  `log_within`. Handling
  SIGINT and SIGTERM belongs with the first long-running loop.
  - **Since milestone 3 (CK-61):** the runtime command handles SIGINT and SIGTERM, as `docs/window-design.md` §6.1 designs. The first
    signal asks for a clean quit (exit 130 or 143); a second ends the process at once with exit 3.

### 4.3 Configuration (Recommendation)

- **Format: TOML.** The owner accepted a data-only file, "yes toml or yaml" (2026-10-18 20:33Z), when ck-guide asked whether
  configuration should be data such as TOML or executable Lua. ck-guide then chose TOML from those two, matching this recommendation.
  YAML support is not requested. Being data, the file is read after the logger starts with its fixed settings (§4.2), before the
  configured logging settings apply and before any Lua VM starts.
- **Source:** built-in defaults, or exactly the file given with `--config`. There is no implicit search of the working directory or the
  environment, so a stray file or variable cannot change a run. This also keeps spec runs deterministic.
- **Schema (milestone 1):**

  ```toml
  [log]
  level = "info"            # error, warn, info, debug, trace
  timestamps = true         # false gives deterministic output for tests
  colour = "auto"           # auto, always, never
  [log.categories]          # per-category overrides, e.g.
  runtime = "debug"
  ```

- Unknown keys are an error that names the key, so typos are not silently ignored. A missing explicit file, a parse error (with its
  line) or an invalid value all exit 1. They are reported as plain `error` records by the logger's fixed start-up settings (§4.2),
  before the configuration's own logging settings apply.

### 4.4 Logging (owner-accepted shape; details are Recommendations)

- **Levels:** `error`, `warn`, `info`, `debug` and `trace`, filtered per category with a global default.
- **Categories:** a short name per record, for example `runtime`, `config`, `lua` and later `metal`.
- **Line format:** `2026-10-18T20:30:00.123Z INFO  runtime: start-up complete`. The timestamp is UTC, computed without a time-zone
  database. With `timestamps = false` the line starts at the level.
- **Colour:** only the level is coloured. `auto` colours only when stdout is a terminal and `NO_COLOR` is unset.
- **Sink (Firm):** stdout only; there is no log file feature.
- **Back-pressure (owner decision, 2026-10-18; replaces the earlier drop-when-full rule).** The owner does not accept dropped log lines
  ("i dont think any dropped lines are acceptable") and chose **"Wait for space"** when the queue is full. So:
  - emitting a record formats it and pushes the whole line onto a **bounded in-memory queue**. The queue's lock is held only to push or
    take records, never during formatting or I/O. A thread that finds the lock briefly held simply waits for it; a thread that finds the
    queue full waits until the writer frees space. **Nothing is dropped** because of contention or a full queue. If stdout is never
    read, that thread waits indefinitely, as any program writing to a full pipe does;
  - **control-path rule (review 7 finding 1):** code that owns deadlines, cancellation or the process exit must never use the waiting
    `log`. That code is the runtime's lifecycle, a supervisor, the executable's main thread and the panic hook. Every record it emits,
    including ordinary progress and failure records, goes through `log_within(deadline)`, which waits like `log` but never past the
    deadline it is given. That deadline is the earlier of now plus a preferred allowance (100 ms by default) and the code's own next
    deadline, which is never extended for logging. If that deadline has already passed, the call makes one non-waiting attempt. `try_log`,
    which never waits, is reserved for code that cannot wait at all.

    `log_within` drops and counts a record whenever it cannot be admitted by its deadline. That includes heavy lock contention and a
    queue that stays full under load even while stdout is healthy, as well as an unread or failed stdout. The preferred allowance makes
    such losses rare but cannot exclude them, so control-path **progress** records are the one place a line can be lost while output
    works. Any such loss makes the run end as an incomplete shutdown (exit 3). The owner accepted this on 2026-10-19, on condition that
    error messages are still captured ("as long as we still capture errors its fine"; recorded as a CK-47 comment).
  - **control-path errors (owner, 2026-10-19; CK-52):**
    - **Reserve:** an error record that `log_within` cannot admit to the queue by its deadline goes to a separate reserve of 1024 slots
      instead. Errors pass every filter, so they never need the settings lock. Publication into the reserve never waits.
    - **Order:** the writer writes reserved errors before its next batch, so an error can appear a few lines ahead of records queued
      before it.
    - **Delivery:** it is tracked per error by sequence number. An error counts as delivered only once it has been written and the sink
      flushed.
    - **Grace at exit:** when the shutdown flush is incomplete and an error is still undelivered, the executable waits once, for at most
      a further 2 s (`ERROR_GRACE`), for errors only. That also applies in the panic hook. A run with no pending error never waits longer
      than before; the owner's words were "a 2 second limit is fine for errors, but i dont want a 2 second pause during normal operation,
      we should just drop the messages".
    - **Remaining limits** (each still exits 3): a stalled or failed stdout; output too slow to deliver the error within the shutdown
      deadline plus the grace; more errors pending than the reserve holds.
    `log_within` never waits on a mutex or condition variable; it polls with try-locks and short parks, because `Condvar::wait_timeout`
    re-acquires its mutex without a bound. `try_log` can also drop on ordinary lock contention, so the control path uses it only where
    no wait at all is acceptable. Ordinary producers (services, Lua VMs, application code) use the waiting `log`;
  - **closure on failure (review 7 finding 2):** if the output fails, the writer closes the queue and wakes every thread waiting for
    space. Each counts its record as lost, since it can no longer be written;
  - lines are lost only when the output itself fails (a write or flush error), through an expired `log_within` or a `try_log`, or for an
    invalid category; every loss is counted;
  - **one writer thread**, owned by the logger, is the only code that writes log lines to stdout. Each line is written whole, so lines
    from different threads never interleave. If records were dropped, the writer reports how many as soon as it can;
  - **flush with a deadline:** shutdown asks the writer to drain and waits up to a fixed deadline. The result is *complete* only when the
    queue is empty **and** the writer has finished writing the record it took last; an empty queue with a write still in progress is
    *incomplete*. An incomplete result carries the number of records still queued, in flight and dropped. A writer still blocked in a
    write is not joined; the process exits without it. Flush stays bounded even while other threads are waiting to log. (Queue capacity
    and the deadline are for the issue spec.)
  - **evidence that can be lost:** records still queued or in flight when an incomplete shutdown ends the process (errors get the bounded
    grace above first), and records lost through `try_log` or a failed output. The final exit status is the only report that cannot be
    lost;
  - **library and executable stay separate:** `ck-log` reports the flush result and `ck-runtime` reports owner outcomes; neither ever
    terminates the process. Only the executable chooses the exit status and how the process ends (review 2 finding 1):
    - **clean completion:** every owner finished and the log flush complete. `main` returns its exit status normally;
    - **incomplete shutdown:** any owner missed its deadline, or the log flush is incomplete. After its bounded best-effort cleanup, the
      executable ends the process with the platform's immediate exit, `_exit(status)`, through `libc` as one documented `unsafe` call.
      This is needed because `std::process::exit` and C `exit` run exit handlers and the C library's stdout cleanup. With a standard
      Lua `print` blocked inside C stdout, or output waiting in its buffer, that cleanup can block again after every deadline has
      passed;
    - **what the incomplete path loses:** queued and in-flight log records, and output still buffered in the C library's stdout (for
      example from `print`). No further diagnostic is attempted through a stream that may be blocked; the distinct exit status is the
      report.
  - **Scope of these guarantees:** lossless admission, whole-line writes and the bounded flush apply only to records emitted through
    the logger (the Rust macros and Lua's `log.*`). Lua's standard `print`, bytes written to stderr after it follows stdout, and any
    native library's writes bypass the queue. They can block, they can interleave with log lines, and they are not counted or flushed by
    the logger.
- **API:** a small native API (`ck_log::info!(category, ...)` style macros over a global logger). An adapter for the common `log` facade
  can be added later if a dependency logs through it.
- **Thread safety:** the logger is a process-wide value that any thread can use, through the queue above (for logger calls
  only; see "Scope of these guarantees"). A record carries its
  category; milestone 2 adds the emitting VM's name (for example `lua.ui` and `lua.gameplay`) so the two VMs' output can be told apart.
- The owner accepted the shape (timestamps, colour, severity, categories, configurable verbosity, stdout only); the exact line format and
  configuration keys above are recommendations for review.

## 5. Milestone 2: parallel Lua VMs (Firm, owner 2026-10-19)

The owner confirmed this section's four recommendations on 2026-10-19, in answers to ck-manager:
- **VM model:** "As recommended", meaning one thread and one Lua state per VM owner, a reusable owner created twice, and channel control
  with deadlines;
- **start failure:** "Stop both, exit 1";
- **runaway Lua:** "Use the hook";
- **`print`:** "Flush per VM".

Message passing between VMs remains **Open**.

**Firm:** the same lifecycle as milestone 1, using the full Lua setup; parallel VMs, one for the UI and one for the gameplay, each with a
skeleton that prints. No real UI or gameplay.

### 5.1 The concurrency boundary

A Lua state is single-threaded: it cannot run Lua on two threads at once, and coroutines inside one state take turns rather than running
in parallel. Serialising two "VMs" through one state or one thread would not be parallel. Hetoimasia reached the same conclusion
(its decisions D-1 and D-15: independently created VMs, each with exactly one execution owner; a general VM owner that an application can
create any number of). That is recorded here as a reference, not copied code.

**Firm (owner 2026-10-19, "As recommended"):**

- **A VM owner** is the `ck-lua` primitive: one OS thread that creates its own Lua state on that thread, runs it, and drops it there. The
  Lua state never crosses threads (the binding's non-`Send` state type enforces this at compile time).
- **Independence:** owners share no mutable Lua state and never call into each other's VM synchronously. The failure of one is reported
  and does not corrupt the other. Each owner has its own name, entry script and set of host functions.
- **Composition, not a fixed pair:** the engine offers a VM owner that an application can create several of. Codekarl's milestone 2 creates
  two, named `ui` and `gameplay`. Nothing in `ck-lua` knows about UI or gameplay.
- **Construction inside the owner, readiness after initialisation:** the owner thread creates the state, installs host functions and loads
  the entry script, and only then reports *ready*.
- **Ownership is tracked from spawn, not from readiness (review 1 finding 2):** the runtime keeps every owner's thread handle from the
  moment the thread is spawned, so an owner that has not yet reported ready is still in the cleanup set. Each owner moves through
  *spawned → initialising → ready → started → stopping → finished*.
  - Each owner also has a **cancel flag** that the runtime can set at any time. The owner checks it between initialisation steps, and the
    instruction hook (§5.2) checks it while Lua code runs.
  - **The owner cleans up what it created.** Its Lua state and anything partially initialised are dropped on the owner's own thread before
    it ends, on every path. The runtime only observes.
  - **Completion is observed, never assumed:** an owner counts as finished only when its thread has actually ended (checked with
    `JoinHandle::is_finished`, then joined). A *stopped* reply alone is not enough. The "owner stopped" record is logged after that
    observation.
- **Control path:** the runtime talks to each owner through a channel (std `mpsc`): `start` and `stop` requests, and replies for success or
  a failure with its message and traceback. Every wait for a reply has a deadline. No message passing between the two VMs is needed for
  this milestone (**Open** for later).
- **Independent cleanup:** stopping or failing one owner never waits on the other owner's Lua code.
- **Shared services:** logging is the only host service both VMs use; it is thread-safe (§4.4). The configuration is passed to each owner
  read-only at creation, as a copy.

### 5.2 Lifecycle with two VMs

1. Milestone 1 steps 1 to 4 (arguments, configuration, logger, start record).
2. Create both owner threads. Each creates its state, installs its host functions and loads its entry script. Both run at the same time.
3. Send `start` to both; each calls its script's `startup` function if defined. The scripts print with standard `print` and log with
   `log.*` (§5.3), meeting the "print from both" requirement. The runtime waits for both replies, each with a deadline.
4. Log that start-up is complete.
5. Send `stop` to both; each calls its script's `shutdown` function if defined, drops its state and ends its thread. The runtime joins both
   threads, each with a deadline.
6. Milestone 1 steps 7 and 8 (reverse-order stop of any other services, clean shutdown, exit 0).

**Failure and cleanup.**

- **Start failures (Firm, owner 2026-10-19, "Stop both, exit 1"):** if any owner fails (thread creation, state creation, script load or
  `startup` error), the runtime logs the error with its traceback, through `log_within` with its current deadline, like every supervisor
  record (§4.4, "control-path rule"). It then sets the cancel flag of **every spawned owner**, whatever its stage, including any not yet
  ready. It waits for each to finish, up to its deadline, and exits 1 under the §4.2 rules. A failure in one owner does not prevent the
  others from being cancelled and joined. An owner that fails its own initialisation drops what it created and ends; the runtime observes
  that it has finished.
- **Panics:** a panic inside an owner thread is caught at the thread boundary and reported as that owner's failure, never as a hang.
- **Deadline missed (not a clean teardown):** a deadline only stops the *runtime* waiting. It does not end a running Lua call and does not
  join the thread. When an owner misses a deadline:
  1. the runtime records it by name as not stopped, through `log_within` (every supervisor record does). Its deadline has already
     passed, so this is one non-waiting attempt, and it never waits on stdout or on a lock the stuck owner might hold;
  2. it still cancels, observes and joins every other owner;
  3. it does not join the stuck thread;
  4. the executable flushes the log with its deadline (§4.4). If an error record is still undelivered, it gives errors the single 2 s
     grace. It then ends the process through the immediate `_exit` path with status 3: the shutdown is **incomplete** (§4.4, "library
     and executable stay separate").

  The stuck owner's `shutdown` function has not run and its state is not dropped; the operating system reclaims the process.
  - **Firm (owner 2026-10-19, "Use the hook"):** reduce how often this happens with a Lua instruction-count hook. On a stop request, or
    after a start deadline, the hook raises an error inside the running script, so pure-Lua loops unwind normally. It cannot interrupt a
    host function or a native write that blocks, such as a `print` stuck on a stalled stdout. That case is the owner missing its deadline,
    reported as an incomplete shutdown, not a clean cancellation.
  - **Firm (owner 2026-10-19, "Wrap pcall when cancelling"):** a hook alone cannot get past `pcall`; CK-55 spec review 4 showed it on
    Lua 5.4.9. So `pcall`, `xpcall`, `coroutine.resume` and `coroutine.close` are wrapped. The wrappers behave exactly like the standard
    functions, except that they re-raise ck-lua's own interruption error, so a cancelled script cannot catch it and continue.

### 5.3 Script surface (minimum)

- **Logging is a separate function (Firm, owner 20:40Z):** `log.error/warn/info/debug/trace(message)` write records in the owner's
  category (`lua.ui` or `lua.gameplay`), with the logger's format, filtering and whole-line atomicity (§4.4).
- **`print` stays standard Lua `print` (Firm, owner 20:40Z).** It is not replaced or redirected. It writes through the C library's stdout as
  Lua always does:
  - its output has no timestamp, level or category;
  - it is not a log record and has none of the logger's guarantees: no whole-record atomicity against log lines or the other VM's
    `print`, no defined order relative to log records, and no protection against a stalled stdout. Like any blocking call, a `print`
    that blocks counts against its owner's deadline (§5.2);
  - **Firm (owner 2026-10-19, "Flush per VM"):** each owner flushes the C library's stdout on its own thread before it finishes, so buffered
    `print` output is not lost at exit, and a flush that blocks is charged to that owner.
- `config` is a read-only table of the loaded configuration.
- Optional global functions `startup()` and `shutdown()`.
- Script errors carry Lua tracebacks.
- The entry scripts for milestone 2 are skeletons, for example `scripts/ui/main.lua` and `scripts/gameplay/main.lua`. Their paths come from
  the configuration (`[lua.ui] entry = ...` and `[lua.gameplay] entry = ...`). Each skeleton prints with `print` and logs with `log`, and
  names its own VM in what it prints, since `print` output carries no category.

### 5.4 Reference: Hetoimasia (read only, not copied)

ck-guide's read-only inspection (2026-10-18 20:32Z; no tests run), checked at source by ck-manager:

- `packages/scripting-lua/README.md` (from line 487): `withVmOwner` gives one VM a protected lifetime on its owner thread, with
  independent Lua globals and registries per owner.
- `packages/scripting-lua/runtime-lua/Hetoimasia/Scripting/Lua/Runtime/Dispatch.hs` (lines 91–138): construction inside the lifetime,
  readiness only after initialisation, stop and admission control, per-owner dispatch.
- `packages/scripting-lua/runtime-lua-test/Test/LuaRuntime/Independence.hs` (from line 17): a steady owner serves 200 calls while other,
  differently configured owners start, retire and fail.
- `docs/lua_runtime_design.md`: D-1 and D-15 as in §5.1. Its LUA-8, full headless UI and gameplay composition, is still **deferred**
  there, so it is not evidence of a finished two-domain runtime.

**Concepts carried over:** an owner primitive, construction and readiness inside the owner, independent state, independent cleanup, and
an independence test. **Not carried over:** Haskell cancellation and FFI mechanisms, Hetoimasia's scheduler, capability policy and
confinement work.

### 5.5 Not assumed

Not assumed (ck-guide's relay): tick or event callbacks, a scheduler, sandboxing or capability limits, hot reload and a mod loader. These
wait for later design. Hetoimasia explores several of these; none is in this epic.

## 6. Dependencies (Recommendation)

| Need | Choice | Rationale |
| --- | --- | --- |
| Logging | none (std) | Small enough to own; timestamps from `SystemTime` with a civil-date conversion; terminal check with `std::io::IsTerminal` |
| Configuration | `toml` with `serde` derive | Widely used, maintained, small surface; `serde` is already in the lockfile through `serde_json` |
| Argument parsing | none (std) | Four options do not justify a parser crate |
| Lua (milestone 2) | `mlua` with Lua 5.4, built from bundled source, without its `send` feature (Firm, owner 20:40Z: Lua 5.4, `mlua`, bundled) | The most used Rust binding; safe API; no system Lua needed; without `send`, a state cannot leave its owner thread |
| Stderr to stdout, immediate exit | `libc` (`dup2`, `_exit`) | Two documented `unsafe` calls in the executable only; std has neither |
| Threads and channels | none (std `thread`, `mpsc`) | Two owners with request/reply channels need nothing more |

- **Alternatives for review:**
  - a hand-written TOML subset, which avoids the `toml` crate but is a parser to maintain;
  - (decided by the owner on 2026-10-18 20:40Z: Lua 5.4 through `mlua`, not LuaJIT or hand-written bindings).
- Every addition goes into `Cargo.lock` and builds with `--locked`, as `tools/check.py` already requires.

## 7. Coexistence with the probes

- The probes stay as flags that bypass the runtime. `--metal-smoke` keeps its exact message, now with stderr following stdout (§4.2), and
  the `gpu_probe` test (its own executable) is untouched.
- The `metal` feature stays optional. The runtime logs enabled features at start but does not create a Metal device in either milestone.
- The existing cases for `--version`, `--help` (updated text) and unknown arguments stay, with usage errors now expected on stdout. The
  no-argument case is removed (§8).

## 8. Validation (Recommendation)

- **`ck-log` unit tests:**
  - level and category filtering, and parsing of the configuration;
  - exact line format with and without timestamps;
  - UTC formatting at known instants (epoch, leap day, year end);
  - colour decisions;
  - a writer that fails: no panic, and shutdown continues.
- **`ck-runtime` unit tests:**
  - services start in order and stop in reverse;
  - a start failure stops only the services already started;
  - a stop failure still stops the rest and sets exit 1;
  - each step is logged exactly once.
- **Synthetic failures stay out of the binary (review 1 finding 4):** start and stop failures from fake services are tested in
  `ck-runtime`'s own tests. The executable's failures are tested only through ordinary inputs: bad configuration and bad scripts. The binary
  gets no test-only service, switch or hook.
- **Exact CLI spec cases (`specs/bootstrap.json`; review 1 finding 3):**
  - **Kept:** the existing fixed-output cases (`--version`, `--help` with its updated text, unknown and extra arguments, `--metal-smoke`).
    Their exact matching and the runner's skip, empty and redirect protections stay as they are.
  - **Changed:** usage errors (unknown and extra arguments) expect `codekarl: <message>` on stdout and nothing on stderr. The runner's
    Metal-notice filter also applies to stdout, so `--metal-smoke` still matches exactly.
  - **Added:** failure cases whose output is fixed: a missing configuration file given as an absolute path that cannot exist (a plain
    `error` record on stdout, and nothing on stderr), and usage errors for `--config` (missing value, repeated option; on stdout, nothing
    on stderr).
  - **Removed:** the no-argument case, because the runtime's default output is timestamped and so cannot be compared byte for byte.
- **Lifecycle tests (`apps/codekarl/tests/runtime.rs`, an application integration test):** this test starts the binary itself, so it can
  pass fixture paths it computes and knows the features it was built with (`cfg!(feature = "metal")`).
  - **Default run (timestamps on):** every line is parsed strictly as timestamp, level, category and message. The test checks the exact
    sequence of records, including the features listed in the start record for the build being tested, and rejects any missing or extra
    record.
  - **Explicit configuration with `timestamps = false` and colour off:** the stdout is compared exactly.
  - **Failure inputs:** malformed configuration (the error names the line), unknown key, invalid level.
  - **The checker is tested too:** unit tests make it reject a malformed timestamp, a missing record, an extra record and the wrong
    feature list.
  - **Both feature sets:** the expected records depend on `cfg!(feature = "metal")`, so the test is correct in either build. Today
    `tools/check.py` runs `cargo test` only with `--all-features`. Covering the feature-less build needs an added test step (**Open for
    the issue spec**). Like the spec runner, the test must not skip silently under `CK_REQUIRE_FULL_SPEC=1`.
  - **No environment override:** nothing in the binary reads a clock override, a feature override or a test-mode variable.
- **Stderr follows stdout:** the application test runs the binary with stdout and stderr connected to two separate pipes. After the
  runtime has started, everything written to stderr (for example a Lua `io.stderr:write` in milestone 2, or a configuration error) must
  arrive on the stdout pipe, and the stderr pipe must stay empty. The same holds for usage errors and for `--metal-smoke`.
- **Stalled stdout (review 1 finding 1):**
  - `ck-log` tests (CK-47): a test-supplied writer that blocks without failing.
    - Ordinary `log` callers wait once the queue is full, and lose nothing when the writer is released.
    - `log_within` and `try_log` drop and count within their bounds.
    - The flush returns *incomplete* within its deadline.
    - A failing writer closes the queue and releases every waiting caller.
  - `ck-runtime` test: with the queue already full behind a stalled sink, a lifecycle progress or failure record still lets the run finish
    within its bound.
  - Application test: the binary is run as a supervised child whose stdout pipe is filled and never read, while the read end stays open.
    The test observes the **child process's own exit status**, the distinct incomplete status, within a whole-process deadline. A
    returned flush result alone is not enough. On failure the supervisor kills and reaps the child within its own bound.
  - Milestone 2 repeats this with both VMs running:
    - a standard Lua `print` blocked inside C stdout (no replacement `print`, no production test mode);
    - output left waiting in the C library's stdout buffer;
    - in each case the child exits with the incomplete status within the deadline;
    - with a healthy stdout, the other owner still reaches its end. With a blocked C stdout, the other owner's `shutdown()` still runs,
      but its own stdout flush can block behind the stuck owner, because C stdout is shared, so it may not finish (CK-58 r4);
    - a progress record logged after a phase deadline may be dropped under load, as the control-path rule allows.
  - A writer that returns an error is tested separately; it is not the same case.
- **Milestone 2 additions:**
  - **Both VMs print and log (review 2 finding 2):** native `print` bytes may legitimately land inside a log line, so a run that mixes
    them cannot be checked line by line. Recommendation for the issue spec:
    - **content** is checked in separate runs: a log-only fixture pair, whose records are parsed per VM in order and tagged `lua.ui` and
      `lua.gameplay`, and a print-only fixture pair, whose raw lines must each appear exactly;
    - the **skeletons**, which both print and log, are checked in a combined run for outcome only: exit status and clean shutdown;
    - exact log formatting is proved in `ck-log`'s own tests;
    - no check discards unknown bytes broadly.

    These checks belong in the application integration test of milestone 1.
  - **Real parallelism (`ck-lua` integration test, not the binary):** two owners are created by the test. One owner's `startup` calls a
    test-registered host function that waits on a barrier until the other owner has started, with a deadline. If both VMs were serialised
    on one thread, the wait would miss its deadline and the test would fail, so passing proves two independent owners. The barrier is
    registered only by the test, so the production binary carries no test hook.
  - **Independence (`ck-lua` integration test):** one owner keeps serving requests while another, configured differently, starts, fails
    and stops. This is the same property Hetoimasia's `Independence.hs` checks (§5.4).
  - **Timeout path (`ck-lua` integration test):** a script in an endless loop is unwound by the instruction hook on stop. A host function
    that blocks past the deadline is reported as incomplete, and the other owner still stops.
  - **Pre-ready cancellation (`ck-lua` integration test; review 1 finding 2):** one owner is held before *ready* by a test-only
    initialisation barrier with a deadline, while its sibling fails during spawn, setup or script load. The held owner must be cancelled,
    drop its state and be observed as finished, or reported incomplete if it misses its deadline. A separate case covers an owner that
    fails its own initialisation.
  - **Failures:** a script error with its traceback (exit 1, the other owner cancelled and joined); a missing entry script; a panic in an
    owner reported as that owner's failure; a stuck owner reported by name after its deadline.
  - `ck-lua` unit tests: the state is created and dropped on its owner thread; host functions log in the owner's category.
  - **`print` and `log` are tested separately:** `ck-lua` checks that `print` is Lua's standard function, not replaced, and that `log.*`
    produces formatted records in the owner's category. The application test checks raw `print` lines and formatted log records as
    two separate expectations, never as one formatted stream. A stalled-stdout case with a blocked `print` expects that owner's
    incomplete-shutdown result.
- `tools/ck check` at full level, including the Metal case, as for every submission.

## 9. Open questions for the owner

1. Later, not this epic: messages between the UI and gameplay VMs, and any scheduling inside a VM.

Answered on 2026-10-18:

- logging expectations (20:28Z; accepted, stdout only);
- the milestone 2 Lua scope (20:28Z; parallel UI and gameplay VMs with printing skeletons);
- the configuration format (20:33Z; data-only, TOML chosen by ck-guide from the owner's "toml or yaml");
- package split, direct `codekarl` start with `--config`, and Lua 5.4 through `mlua` (20:40Z; "all correct as recommended");
- `print` stays standard with a separate log function, and stderr follows stdout (20:40Z), applied to every command with no runtime-only
  exception (ck-guide, 20:45Z, from the owner's unqualified statement).

## 10. Out of scope

Simulation loop, windowing, rendering, ECS, resource manager, map representation, multiplayer, mod loading, hot reload, Lua sandboxing,
and any gameplay system.

## Provenance

Drafted by ck-manager (Claude Opus 5.5, Class S, high effort) on 2026-10-18 from:

- the owner's messages of 2026-10-17 16:43Z, 2026-10-18 20:23Z, 20:28Z, 20:33Z and 20:40Z in the ck-guide conversation `01a14231…`;
- ck-guide relays `5tr2jybfqeqiz7ciruhdkmf5ms`, `6mqi4ykgbp8mfxqq86iuz2mwjs`, `pbawyp25qt9dduf34i5s6pp4mn`, `h3g3hhexkfqywe95iq47ihvvvn` and
  `iph8xy6ebmncwikbtcxunhxb3w`;
- Hetoimasia's `docs/lua_runtime_design.md` decisions D-1 and D-15, read as a reference only;
- the accepted repository state.

Handoff: `.pipeline/coordination/runtime-design/handoff.md`.
