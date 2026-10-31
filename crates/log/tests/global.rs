//! The process-wide logger. One test, because the installed logger is per process.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::time::Instant;

use ck_log::{Admission, Filter, FlushOutcome, Level, Logger, Settings, install, installed};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("buffer lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn logger(buffer: &Buffer, level: Level) -> Logger {
    let settings = Settings {
        filter: Filter::new(level),
        timestamps: false,
        colour: false,
    };
    Logger::new(Box::new(buffer.clone()), settings).expect("the writer thread starts")
}

#[test]
fn macros_do_nothing_before_install_and_install_happens_once() {
    assert!(installed().is_none());
    // Before anything is installed the macros do nothing, whatever their arguments.
    ck_log::error!("early", "not shown {}", 1);
    ck_log::trace!("early", "not shown");
    // The bounded and non-waiting macros lose nothing before install, and say so.
    let soon = Instant::now() + Duration::from_secs(5);
    assert_eq!(
        ck_log::log_within!(Level::Error, soon, "early", "not shown {}", 2),
        Admission::Filtered
    );
    assert_eq!(
        ck_log::try_log!(Level::Error, "early", "not shown"),
        Admission::Filtered
    );

    let first = Buffer::default();
    install(logger(&first, Level::Debug)).expect("must be Ok or Some");

    let second = Buffer::default();
    let refused = install(logger(&second, Level::Trace)).expect_err("must be rejected");
    assert_eq!(refused.to_string(), "a global logger is already installed");

    let name = "world";
    ck_log::error!("runtime", "e {name}");
    ck_log::warn!("runtime", "w {}", 2);
    ck_log::info!("lua.ui", "i");
    ck_log::debug!("runtime", "d");
    ck_log::trace!("runtime", "t is below the installed level");
    // The control-path macro waits at most to its deadline, and reports what happened.
    assert_eq!(
        ck_log::log_within!(Level::Warn, soon, "runtime", "w within {}", 3),
        Admission::Queued
    );
    assert_eq!(
        ck_log::log_within!(Level::Trace, soon, "runtime", "below the installed level"),
        Admission::Filtered
    );
    assert_eq!(
        ck_log::try_log!(Level::Trace, "runtime", "below the installed level"),
        Admission::Filtered
    );

    let installed = installed().expect("a logger is installed");
    assert_eq!(
        installed.flush(Duration::from_secs(5)),
        FlushOutcome::Complete
    );
    let written = String::from_utf8(first.0.lock().expect("must be Ok or Some").clone())
        .expect("must be Ok or Some");
    assert_eq!(
        written,
        "ERROR runtime: e world\nWARN  runtime: w 2\nINFO  lua.ui: i\nDEBUG runtime: d\n\
         WARN  runtime: w within 3\n"
    );
    assert!(second.0.lock().expect("must be Ok or Some").is_empty());

    installed.set_settings(Settings {
        filter: Filter::new(Level::Trace),
        timestamps: false,
        colour: false,
    });
    ck_log::trace!("runtime", "now shown");
    assert_eq!(
        installed.flush(Duration::from_secs(5)),
        FlushOutcome::Complete
    );
    let written = String::from_utf8(first.0.lock().expect("must be Ok or Some").clone())
        .expect("must be Ok or Some");
    assert!(written.ends_with("TRACE runtime: now shown\n"), "{written}");
}
