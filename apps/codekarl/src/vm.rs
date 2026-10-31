//! The configured Lua VMs as supervised owners, and the exit status of a supervised run.
//!
//! [`VmAdapter`] is a local wrapper around a shared [`ck_lua::VmOwner`] that implements
//! [`ck_runtime::Owner`] with `ck-lua`'s split request and wait calls. A wrapper, because both the trait
//! and the type are foreign, so Rust's orphan rule forbids implementing one directly on the other. The
//! handle is an `Rc<RefCell<..>>` (see [`crate::ui_link`]): the VM named `ui` shares it with the UI link,
//! on the main thread only, and the two never use it at the same time.
//!
//! **The VM named `ui`** (fixed, not configurable) is created with the `window`, `render`, `texture` and `frame` extensions,
//! a call limit of 4, the notifier's `wake` as its report wake and the completion hook that publishes the scene; no other
//! VM gets any of them.
//!
//! [`status_of`] maps a [`Report`] to the exit status (design §4.4, §3.4 of the window issue): an owner
//! that missed a deadline, or an `Unproven` window teardown, makes the shutdown incomplete, *whether or not
//! it later finished*, so it is status 3 and ends the process at once; otherwise a failed start, run or
//! stop, or a late callback failure, is status 1; otherwise 0 (the signal statuses are applied after the
//! flush, below 3 and 1).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use ck_lua::{ConfigValue, Extension, VmError, VmOwner, VmSettings};
use ck_render_2d::SceneBridge;
use ck_runtime::{Outcome, Owner, OwnerError, OwnerSpawn, Progress, Report};

use crate::config::VmConfig;
use crate::lifecycle::Status;
use crate::ui_link::{UiSlot, UiVm};

/// The name of the UI VM: the only one with the window API.
pub(crate) const UI_VM: &str = "ui";
/// The calls the UI VM may have in flight (design §4.2: at most two events are ever pending).
pub(crate) const UI_CALL_LIMIT: usize = 4;

/// What the UI VM is given beyond the standard host surface.
pub(crate) struct UiWiring {
    /// The extensions: `window` (possibly extended by a test), `render`, `texture` and `frame`.
    pub(crate) extensions: Vec<Extension>,
    /// Where the spawn puts the shared handle for the UI link.
    pub(crate) slot: UiSlot,
    /// The report wake: the notifier's `wake`.
    pub(crate) wake: Arc<dyn Fn() + Send + Sync>,
    /// The run's scene bridge: every owner's cancellation ends it first (see [`VmAdapter`]).
    pub(crate) bridge: Arc<SceneBridge>,
    /// Run notes used to identify owners that may retain textures at teardown.
    pub(crate) notes: Rc<crate::windowed::RunNotes>,
    /// The completion hook of the UI VM: it publishes the scene after startup and after every posted callback. The
    /// gameplay VM never gets one.
    pub(crate) on_complete: Option<Arc<dyn Fn(ck_lua::Completion) + Send + Sync>>,
}

/// A shared [`VmOwner`] that `ck-runtime` can supervise.
pub(crate) struct VmAdapter {
    name: String,
    vm: UiVm,
    /// The run's scene bridge, ended before a cancellation is forwarded (a failed or aborted start-up cancels
    /// every owner and never asks one to stop, so no `shutdown()` runs there; a host call that is still decoding
    /// then finishes into an ended bridge, and its write is discarded and counted).
    bridge: Option<Arc<SceneBridge>>,
    notes: Option<Rc<crate::windowed::RunNotes>>,
}

/// What the runtime is told about a `ck-lua` error: a missed deadline stays a deadline, and anything
/// else is a failure whose message may span several lines (a Lua error carries its traceback).
pub(crate) fn owner_error(error: VmError) -> OwnerError {
    match error {
        VmError::Deadline { .. } => OwnerError::Deadline,
        VmError::Script {
            message, traceback, ..
        } => OwnerError::Failed {
            message: format!("{message}\n{traceback}"),
        },
        VmError::Panicked { message, .. } => OwnerError::Failed {
            message: format!("panicked: {message}"),
        },
        other @ (VmError::Cancelled { .. }
        | VmError::Spawn { .. }
        | VmError::Disconnected { .. }
        | VmError::Full { .. }
        | VmError::State { .. }) => OwnerError::Failed {
            message: other.to_string(),
        },
    }
}

/// `ck-lua`'s poll answer as the runtime's.
fn progress(answer: ck_lua::Progress) -> Progress {
    match answer {
        ck_lua::Progress::Pending => Progress::Pending,
        ck_lua::Progress::Done => Progress::Done,
    }
}

impl Owner for VmAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn wait_ready(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.vm.borrow_mut().wait_ready(within).map_err(owner_error)
    }

    fn poll_ready(&mut self, within: Duration) -> Result<Progress, OwnerError> {
        self.vm
            .borrow_mut()
            .poll_ready(within)
            .map(progress)
            .map_err(owner_error)
    }

    fn request_start(&mut self) -> Result<(), OwnerError> {
        self.vm.borrow_mut().request_start().map_err(owner_error)
    }

    fn await_started(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.vm
            .borrow_mut()
            .await_started(within)
            .map_err(owner_error)
    }

    fn poll_started(&mut self, within: Duration) -> Result<Progress, OwnerError> {
        self.vm
            .borrow_mut()
            .poll_started(within)
            .map(progress)
            .map_err(owner_error)
    }

    fn request_stop(&mut self) -> Result<(), OwnerError> {
        self.vm.borrow_mut().request_stop().map_err(owner_error)
    }

    fn await_stopped(&mut self, within: Duration) -> Result<(), OwnerError> {
        self.vm
            .borrow_mut()
            .await_stopped(within)
            .map_err(owner_error)
    }

    fn cancel(&mut self) {
        // The cancellation of any owner ends the run's bridge first, `ui` or not: the scene is the run's.
        if let Some(bridge) = &self.bridge {
            bridge.detach();
        }
        self.vm.borrow_mut().cancel();
    }

    fn wait_finished(&mut self, within: Duration) -> bool {
        let finished = self.vm.borrow_mut().wait_finished(within);
        note_unfinished_holder(&self.name, finished, self.notes.as_deref());
        finished
    }
}

/// The test seam for the actual `wait_finished` observation: a timed-out UI owner remains a named resource holder.
pub(crate) fn note_unfinished_holder(
    name: &str,
    finished: bool,
    notes: Option<&crate::windowed::RunNotes>,
) {
    if !finished
        && name == UI_VM
        && let Some(notes) = notes
    {
        notes.retain_resource_holder("UI VM");
    }
}

/// One [`OwnerSpawn`] per configured VM, in the order given. Each VM sees `config`, the whole
/// configuration file, as its `config` table. The VM named [`UI_VM`] is given `ui` (when there is one).
pub(crate) fn spawns(
    vms: &[VmConfig],
    config: &ConfigValue,
    ui: Option<&UiWiring>,
) -> Vec<OwnerSpawn> {
    vms.iter()
        .map(|vm| {
            let mut settings = VmSettings::new(vm.name.clone(), vm.entry.clone(), config.clone());
            let slot = ui.filter(|_| vm.name == UI_VM).map(|wiring| {
                settings.extensions.clone_from(&wiring.extensions);
                settings.call_limit = UI_CALL_LIMIT;
                settings.on_report = Some(Arc::clone(&wiring.wake));
                settings.on_complete.clone_from(&wiring.on_complete);
                Rc::clone(&wiring.slot)
            });
            let name = vm.name.clone();
            let bridge = ui.map(|wiring| Arc::clone(&wiring.bridge));
            let notes = ui.map(|wiring| Rc::clone(&wiring.notes));
            OwnerSpawn::new(vm.name.clone(), move || {
                VmOwner::spawn(settings)
                    .map(|owner| {
                        let shared: UiVm = Rc::new(RefCell::new(owner));
                        if let Some(slot) = slot {
                            *slot.borrow_mut() = Some(Rc::clone(&shared));
                        }
                        Box::new(VmAdapter {
                            name,
                            vm: shared,
                            bridge,
                            notes,
                        }) as Box<dyn Owner>
                    })
                    .map_err(|error| error.to_string())
            })
        })
        .collect()
}

/// The exit status of a supervised run. **3, forced immediate:** `missed` or `unfinished` is non-empty, or
/// the window teardown was `Unproven` (`unproven_window`). **1:** otherwise, a start, run or stop failed,
/// or a callback failed late (`late_failure`, found after the stop phase). **0:** otherwise.
pub(crate) fn status_of(report: &Report, unproven_window: bool, late_failure: bool) -> Status {
    let owners_incomplete =
        !report.missed.is_empty() || !report.unfinished.is_empty() || unproven_window;
    let code = match report.outcome {
        Outcome::Clean if late_failure => 1,
        Outcome::Clean => 0,
        Outcome::StartFailed { .. } | Outcome::RunFailed { .. } | Outcome::StopFailed { .. } => 1,
    };
    Status {
        code,
        owners_incomplete,
    }
}

#[cfg(test)]
mod tests {
    use ck_runtime::{Failure, ServiceError};

    use super::*;

    #[test]
    fn an_unfinished_ui_owner_is_named_by_the_wait_finished_path() {
        let notes = crate::windowed::RunNotes::default();
        note_unfinished_holder(UI_VM, false, Some(&notes));
        assert_eq!(&*notes.resource_holders.borrow(), &["UI VM"]);
        note_unfinished_holder("gameplay", false, Some(&notes));
        note_unfinished_holder(UI_VM, true, Some(&notes));
        assert_eq!(&*notes.resource_holders.borrow(), &["UI VM"]);
    }

    fn report(outcome: Outcome, missed: &[&str], unfinished: &[&str]) -> Report {
        Report {
            outcome,
            missed: missed.iter().map(|name| (*name).to_owned()).collect(),
            unfinished: unfinished.iter().map(|name| (*name).to_owned()).collect(),
            quit: false,
        }
    }

    fn start_failed() -> Outcome {
        Outcome::StartFailed {
            service: "ui".to_owned(),
            error: ServiceError::new("no"),
            stop_failures: Vec::new(),
        }
    }

    fn run_failed() -> Outcome {
        Outcome::RunFailed {
            error: ServiceError::new("the window broke"),
            stop_failures: Vec::new(),
        }
    }

    fn stop_failed() -> Outcome {
        Outcome::StopFailed {
            failures: vec![Failure {
                service: "ui".to_owned(),
                error: ServiceError::new("no"),
            }],
        }
    }

    #[test]
    fn a_clean_report_is_status_0_and_returns_normally() {
        assert_eq!(
            status_of(&report(Outcome::Clean, &[], &[]), false, false),
            Status {
                code: 0,
                owners_incomplete: false
            }
        );
    }

    #[test]
    fn a_failed_start_or_stop_without_a_miss_is_status_1_and_returns_normally() {
        for outcome in [start_failed(), run_failed(), stop_failed()] {
            assert_eq!(
                status_of(&report(outcome, &[], &[]), false, false),
                Status {
                    code: 1,
                    owners_incomplete: false
                }
            );
        }
    }

    #[test]
    fn any_missed_or_unfinished_owner_forces_the_immediate_exit_whatever_the_outcome() {
        // Every combination of an outcome with a miss, an unfinished owner, or both: the code is the
        // outcome's, and `owners_incomplete` is what makes `lifecycle` end the process at once with 3.
        for (outcome, code) in [
            (Outcome::Clean, 0),
            (start_failed(), 1),
            (run_failed(), 1),
            (stop_failed(), 1),
        ] {
            for (missed, unfinished) in [
                (&["ui"][..], &[][..]),
                (&[][..], &["ui"][..]),
                (&["ui"][..], &["ui"][..]),
                (&["gameplay", "ui"][..], &["ui"][..]),
            ] {
                assert_eq!(
                    status_of(&report(outcome.clone(), missed, unfinished), false, false),
                    Status {
                        code,
                        owners_incomplete: true
                    },
                    "{outcome:?} missed {missed:?} unfinished {unfinished:?}"
                );
            }
        }
    }

    #[test]
    fn a_missed_deadline_that_recovered_still_counts_a_clean_outcome_as_incomplete() {
        // `missed` names an owner that missed a deadline even though it was then observed finished
        // (`unfinished` is empty): design §4.4 makes that an incomplete shutdown too.
        let recovered = report(Outcome::Clean, &["ui"], &[]);
        assert!(status_of(&recovered, false, false).owners_incomplete);
    }

    #[test]
    fn lua_errors_become_owner_errors_that_keep_their_text() {
        let script = VmError::Script {
            owner: "ui".to_owned(),
            message: "main.lua:3: boom".to_owned(),
            traceback: "stack traceback:\n\t[C]: in ?".to_owned(),
        };
        assert_eq!(
            owner_error(script),
            OwnerError::Failed {
                message: "main.lua:3: boom\nstack traceback:\n\t[C]: in ?".to_owned()
            }
        );
        assert_eq!(
            owner_error(VmError::Deadline {
                owner: "ui".to_owned()
            }),
            OwnerError::Deadline
        );
        assert_eq!(
            owner_error(VmError::Panicked {
                owner: "ui".to_owned(),
                message: "oops".to_owned()
            }),
            OwnerError::Failed {
                message: "panicked: oops".to_owned()
            }
        );
        for other in [
            VmError::Cancelled {
                owner: "ui".to_owned(),
            },
            VmError::Spawn {
                owner: "ui".to_owned(),
                message: "no threads".to_owned(),
            },
        ] {
            let text = other.to_string();
            assert_eq!(owner_error(other), OwnerError::Failed { message: text });
        }
    }

    #[test]
    fn status_3_wins_then_1_and_an_unproven_window_or_a_late_failure_is_never_clean() {
        let clean = report(Outcome::Clean, &[], &[]);
        // Unproven: incomplete, whatever else happened.
        for (outcome, code) in [(Outcome::Clean, 0), (run_failed(), 1), (stop_failed(), 1)] {
            assert_eq!(
                status_of(&report(outcome, &[], &[]), true, false),
                Status {
                    code,
                    owners_incomplete: true
                }
            );
        }
        // A late callback failure makes a clean run status 1, never 0, and is not an incomplete shutdown.
        assert_eq!(
            status_of(&clean, false, true),
            Status {
                code: 1,
                owners_incomplete: false
            }
        );
        // Both: still incomplete (3 wins) and 1 underneath.
        assert_eq!(
            status_of(&clean, true, true),
            Status {
                code: 1,
                owners_incomplete: true
            }
        );
    }

    #[test]
    fn the_exit_status_of_every_combination_follows_3_then_1_then_the_signal_then_0() {
        use std::cell::Cell;
        use std::sync::OnceLock;

        use ck_log::FlushOutcome;

        use crate::lifecycle::conclude;

        let outcomes = [Outcome::Clean, run_failed(), stop_failed(), start_failed()];
        for outcome in outcomes {
            for unproven in [false, true] {
                for late in [false, true] {
                    for missed in [false, true] {
                        for signal in [None, Some(130), Some(143)] {
                            let miss: &[&str] = if missed { &["ui"] } else { &[] };
                            let status =
                                status_of(&report(outcome.clone(), miss, &[]), unproven, late);
                            let exited = Cell::new(None);
                            let returned = conclude(
                                status,
                                &OnceLock::new(),
                                |_| FlushOutcome::Complete,
                                || {},
                                |code| exited.set(Some(code)),
                                || signal,
                            );
                            let failed = !matches!(outcome, Outcome::Clean) || late;
                            let context = format!(
                                "{outcome:?} unproven {unproven} late {late} missed {missed} signal {signal:?}"
                            );
                            if unproven || missed {
                                // The incomplete shutdown ends the process at once: 3 over a callback
                                // failure (1) and over a signal (130, 143).
                                assert_eq!((returned, exited.get()), (3, Some(3)), "{context}");
                            } else if failed {
                                assert_eq!((returned, exited.get()), (1, None), "{context}");
                            } else {
                                assert_eq!(
                                    (returned, exited.get()),
                                    (signal.unwrap_or(0), None),
                                    "{context}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A scratch directory with scripts for the supervised runs below.
    fn scripts(ui: &str, gameplay: &str) -> (std::path::PathBuf, Vec<VmConfig>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "codekarl-vm-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        std::fs::write(dir.join("ui.lua"), ui).expect("the ui script");
        std::fs::write(dir.join("gameplay.lua"), gameplay).expect("the gameplay script");
        let vms = vec![
            VmConfig {
                name: "gameplay".to_owned(),
                entry: dir.join("gameplay.lua"),
            },
            VmConfig {
                name: "ui".to_owned(),
                entry: dir.join("ui.lua"),
            },
        ];
        (dir, vms)
    }

    #[test]
    fn only_the_ui_vm_gets_the_window_api_a_call_limit_of_4_and_the_report_wake() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use ck_runtime::{RunEnd, Runtime};
        use ck_window::{Notifier, Requests, UiEvent, UiLink};

        use crate::ui_link::{CodekarlUi, UiSlot};

        let (dir, vms) = scripts(
            "assert(type(window) == 'table', 'the ui VM has the window API')\n\
             window.on_opened(function() end)",
            "assert(window == nil, 'the gameplay VM has no window API')",
        );
        let requests = Requests::new();
        let wakes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&wakes);
        let slot = UiSlot::default();
        let wiring = UiWiring {
            extensions: vec![crate::window_api::extension(
                requests.writer(Notifier::new()),
                None,
            )],
            slot: Rc::clone(&slot),
            wake: Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
            bridge: SceneBridge::new(ck_render_2d::Filter::Nearest),
            notes: Rc::new(crate::windowed::RunNotes::default()),
            on_complete: None,
        };
        let spawned = spawns(
            &vms,
            &ConfigValue::Table(std::collections::BTreeMap::new()),
            Some(&wiring),
        );
        let ui = CodekarlUi::new(slot);
        let run_link = ui.clone();
        let refused = Rc::new(std::cell::Cell::new(None));
        let seen = Rc::clone(&refused);
        let report = Runtime::new(Vec::new())
            .with_owners(spawned)
            .with_run(move |_| {
                let mut link = run_link;
                // Four calls fit; a fifth, with no result taken yet, is refused at once.
                for _ in 0..4 {
                    link.post(UiEvent::Opened).expect("within the limit of 4");
                }
                seen.set(Some(link.post(UiEvent::Opened)));
                RunEnd::Finished
            })
            .run_report();
        assert_eq!(report.outcome, Outcome::Clean, "{report:?}");
        assert!(report.missed.is_empty() && report.unfinished.is_empty());
        assert_eq!(refused.get(), Some(Err(ck_window::PostError::Full)));
        assert!(
            wakes.load(Ordering::SeqCst) >= 3,
            "the ui VM's reports woke the notifier"
        );
        // Without the wiring, no VM gets the window API: the ui script's assertion fails its start.
        let spawned = spawns(
            &vms,
            &ConfigValue::Table(std::collections::BTreeMap::new()),
            None,
        );
        let report = Runtime::new(Vec::new()).with_owners(spawned).run_report();
        assert!(
            matches!(report.outcome, Outcome::StartFailed { .. }),
            "{report:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn one_spawn_is_made_per_configured_vm_in_the_given_order() {
        let vms = [
            VmConfig {
                name: "gameplay".to_owned(),
                entry: "g.lua".into(),
            },
            VmConfig {
                name: "ui".to_owned(),
                entry: "u.lua".into(),
            },
        ];
        let config = ConfigValue::Table(std::collections::BTreeMap::new());
        assert_eq!(spawns(&vms, &config, None).len(), 2);
        assert!(spawns(&[], &config, None).is_empty());
    }

    /// A VM owner of `script`, spawned but not waited for, wrapped as the runtime's owner over `bridge`.
    fn adapter_of(
        name: &str,
        extensions: Vec<Extension>,
        script: &str,
        bridge: &Arc<SceneBridge>,
    ) -> (VmAdapter, crate::testing::Dir) {
        let dir = crate::testing::Dir::new();
        let entry = dir.0.join("main.lua");
        std::fs::write(&entry, script).expect("a script");
        let mut settings = VmSettings::new(
            name,
            entry,
            ConfigValue::Table(std::collections::BTreeMap::new()),
        );
        settings.extensions = extensions;
        let owner = VmOwner::spawn(settings).expect("the VM spawns");
        let adapter = VmAdapter {
            name: name.to_owned(),
            vm: Rc::new(RefCell::new(owner)),
            bridge: Some(Arc::clone(bridge)),
            notes: None,
        };
        (adapter, dir)
    }

    #[test]
    fn cancelling_any_owner_ends_the_bridge_and_an_in_flight_decode_is_discarded_and_counted() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("small.png"),
            crate::testing::png(8, 4, &[255; 8 * 4 * 4]),
        )
        .expect("an image");
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let writer = bridge.take_writer().expect("the one writer");
        let (gate, entered) = (
            crate::testing::Gate::new(),
            Arc::new(AtomicBool::new(false)),
        );
        let hold: Box<dyn Fn() + Send + Sync> = {
            let (gate, entered) = (Arc::clone(&gate), Arc::clone(&entered));
            Box::new(move || {
                entered.store(true, Ordering::SeqCst);
                gate.wait();
            })
        };
        let render = crate::render_api::extension_holding(Some(assets.0.clone()), writer, hold);
        // The ui VM's main chunk is decoding (held between the decode and the write) when the start-up of
        // another owner fails and the runtime cancels every owner.
        let marker = assets.0.join("shutdown-ran");
        let (mut ui, _dir) = adapter_of(
            "ui",
            vec![render],
            &format!(
                "render.quad{{image='small.png'}}\nfunction shutdown() io.open([==[{}]==], 'w'):close() end",
                marker.display()
            ),
            &bridge,
        );
        let give_up = std::time::Instant::now() + Duration::from_secs(10);
        while !entered.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < give_up,
                "the decode never began"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(bridge.phase(), ck_render_2d::Phase::Before, "still live");
        // Any owner's cancellation ends the run's bridge: here another owner's.
        let (mut other, _other_dir) = adapter_of("gameplay", Vec::new(), "", &bridge);
        other.cancel();
        assert_eq!(bridge.phase(), ck_render_2d::Phase::Ended);
        ui.cancel();
        gate.open();
        assert!(
            ui.wait_finished(Duration::from_secs(10)),
            "the cancelled VM ended"
        );
        let counts = bridge.counts();
        assert_eq!(
            (counts.accepted, counts.discarded),
            (1, 1),
            "the write that finished after the end was discarded and counted"
        );
        assert!(!marker.exists(), "a cancelled VM runs no shutdown()");
    }

    /// An aborted start-up through the real runtime: the ui VM's main chunk is held (a decode in flight) past the
    /// start deadline, so the runtime abandons the start and cancels every owner. The adapters end the bridge
    /// before they forward the cancel; the held decode then finishes into the ended bridge (the gate is opened by
    /// a thread that waits for the end) and its write is discarded and counted. No `shutdown()` runs.
    #[test]
    fn an_aborted_start_up_ends_the_bridge_before_cancelling_and_a_held_decode_is_discarded() {
        use ck_runtime::Runtime;

        let assets = crate::testing::Dir::new();
        std::fs::write(
            assets.0.join("small.png"),
            crate::testing::png(8, 4, &[255; 8 * 4 * 4]),
        )
        .expect("an image");
        let marker = assets.0.join("shutdown-ran");
        let (dir, vms) = scripts(
            &format!(
                "render.quad{{image='small.png'}}\nfunction shutdown() io.open([==[{}]==], 'w'):close() end",
                marker.display()
            ),
            "function shutdown() end",
        );
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let writer = bridge.take_writer().expect("the one writer");
        let gate = crate::testing::Gate::new();
        let hold: Box<dyn Fn() + Send + Sync> = {
            let gate = Arc::clone(&gate);
            Box::new(move || gate.wait())
        };
        let wiring = UiWiring {
            extensions: vec![crate::render_api::extension_holding(
                Some(assets.0.clone()),
                writer,
                hold,
            )],
            slot: UiSlot::default(),
            wake: Arc::new(|| {}),
            bridge: Arc::clone(&bridge),
            notes: Rc::new(crate::windowed::RunNotes::default()),
            on_complete: None,
        };
        let spawned = spawns(
            &vms,
            &ConfigValue::Table(std::collections::BTreeMap::new()),
            Some(&wiring),
        );
        let opener = {
            let (gate, bridge) = (Arc::clone(&gate), Arc::clone(&bridge));
            std::thread::spawn(move || {
                let give_up = std::time::Instant::now() + Duration::from_secs(20);
                while bridge.phase() != ck_render_2d::Phase::Ended
                    && std::time::Instant::now() < give_up
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                gate.open();
            })
        };
        let report = Runtime::new(Vec::new())
            .with_owners(spawned)
            .with_owner_deadlines(
                Duration::from_millis(500),
                Duration::from_secs(5),
                Duration::from_secs(10),
            )
            .run_report();
        let _ = opener.join();
        assert!(
            !matches!(report.outcome, Outcome::Clean),
            "the start-up was abandoned: {report:?}"
        );
        assert!(report.unfinished.is_empty(), "{report:?}");
        let counts = bridge.counts();
        assert_eq!(
            (counts.accepted, counts.discarded),
            (1, 1),
            "the held decode finished into an ended bridge"
        );
        assert!(!marker.exists(), "an abandoned start-up runs no shutdown()");
        let _ = std::fs::remove_dir_all(dir);
    }
}
