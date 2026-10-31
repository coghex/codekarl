//! Fixtures shared by the scenario tests.
//!
//! Every scenario is its own test file, so it runs in its own process and installs its own global
//! `ck_log` logger over an in-memory sink. Scenarios read the records only after a bounded `flush`
//! that must be `Complete`; a lossy or incomplete logger fails the test instead of being retried.
//!
//!
//! The sink, the logger and the way the runtime is driven are in `harness`.

use std::sync::{Arc, Mutex};

use ck_runtime::{Outcome, Runtime, Service, ServiceError};

#[path = "../harness/mod.rs"]
#[expect(
    dead_code,
    reason = "the failure injection is used only by the `owners` scenarios, which include the harness directly"
)]
mod harness;

pub(crate) use harness::{Output, install};

/// A service that records its calls and can be told to fail.
pub(crate) struct Spec {
    pub(crate) name: &'static str,
    pub(crate) fail_start: bool,
    pub(crate) fail_stop: bool,
}

impl From<&'static str> for Spec {
    /// A service that starts and stops without error.
    fn from(name: &'static str) -> Self {
        Self {
            name,
            fail_start: false,
            fail_stop: false,
        }
    }
}

/// What one run of the runtime did.
pub(crate) struct Run {
    pub(crate) outcome: Outcome,
    /// The service calls made, as `start <name>` and `stop <name>`, in order.
    pub(crate) calls: Vec<String>,
    /// The log records this run wrote, as lines, in order.
    pub(crate) records: Vec<String>,
}

struct Fake {
    spec: Spec,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Service for Fake {
    fn name(&self) -> &str {
        self.spec.name
    }

    fn start(&mut self) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .push(format!("start {}", self.spec.name));
        if self.spec.fail_start {
            Err(ServiceError::new(format!(
                "{} refused to start",
                self.spec.name
            )))
        } else {
            Ok(())
        }
    }

    fn stop(&mut self) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .expect("the call log is not poisoned")
            .push(format!("stop {}", self.spec.name));
        if self.spec.fail_stop {
            Err(ServiceError::new(format!(
                "{} refused to stop",
                self.spec.name
            )))
        } else {
            Ok(())
        }
    }
}

impl Output {
    /// Runs a runtime over fake services built from `specs`, waits (bounded) for the logger to write
    /// everything, and returns the outcome, the service calls and the records written by this run.
    pub(crate) fn run(&self, specs: Vec<Spec>) -> Run {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let services: Vec<Box<dyn Service>> = specs
            .into_iter()
            .map(|spec| {
                Box::new(Fake {
                    spec,
                    calls: Arc::clone(&calls),
                }) as Box<dyn Service>
            })
            .collect();
        let (report, records) = self.drive(Runtime::new(services));
        let calls = calls.lock().expect("the call log is not poisoned").clone();
        Run {
            outcome: report.outcome,
            calls,
            records,
        }
    }
}
