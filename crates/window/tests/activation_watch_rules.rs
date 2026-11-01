//! Platform-free activation-watch rule and forwarding-format tests.

#[path = "support/activation_watch.rs"]
mod activation_watch;

fn ordinary(lines: &[&str]) -> Result<Option<String>, String> {
    let lines = lines
        .iter()
        .map(|line| (*line).to_owned())
        .collect::<Vec<_>>();
    activation_watch::classify(&lines, false).map(|result| result.external_line)
}

#[test]
fn ordinary_outcome_and_end_state_only_are_external() {
    assert!(
        ordinary(&[
            "installed=1",
            "pid=71",
            "outcome=becameActive",
            "end_active=0 end_key=0 end_frontmost=0"
        ])
        .unwrap()
        .unwrap()
        .contains("pid=71 becameActive=1")
    );
    assert!(
        ordinary(&[
            "installed=1",
            "pid=72",
            "end_active=0 end_key=1 end_frontmost=1"
        ])
        .unwrap()
        .unwrap()
        .contains("end_key=1 end_frontmost=1")
    );
}

#[test]
fn requests_exposure_and_missing_watch_evidence_fail_closed() {
    for request in [
        "activate",
        "activateIgnoringOtherApps",
        "makeKeyAndOrderFront",
        "runningApplicationActivate",
        "runningApplicationActivateFrom",
    ] {
        assert!(ordinary(&["installed=1", &format!("request={request}")]).is_err());
    }
    assert!(ordinary(&["installed=1", "request=activate", "outcome=becameKey"]).is_err());
    assert!(ordinary(&["installed=1", "exposed=44"]).is_err());
    assert!(ordinary(&[]).is_err());
    assert!(ordinary(&["installed=1"]).is_err());
    assert!(activation_watch::classify(&["outcome=becameKey".to_owned()], false).is_err());
}

#[test]
fn report_without_end_state_from_a_paused_or_signalled_worker_is_classified() {
    let path = std::env::temp_dir().join(format!(
        "ck-activation-watch-no-end-state-{}.txt",
        std::process::id()
    ));
    std::fs::write(&path, "pid=4242\nexpected_request=0\ninstalled=1\n").unwrap();

    activation_watch::assert_report(&path, "native.window", "paused_worker", "quiet");
    std::fs::remove_file(path).unwrap();

    let with_request = [
        "pid=4242".to_owned(),
        "expected_request=0".to_owned(),
        "installed=1".to_owned(),
        "request=activate".to_owned(),
    ];
    assert_eq!(
        activation_watch::classify(&with_request, false).unwrap_err(),
        "activation request was made: activate"
    );
    assert!(
        activation_watch::classify(&["pid=4242".to_owned(), "installed=1".to_owned(),], false)
            .is_err(),
        "only an ordinary worker report may omit the end state"
    );
}

#[test]
fn expected_request_requires_suppressed_activation_and_key_requests() {
    let passing = vec![
        "installed=1".to_owned(),
        "request=activate".to_owned(),
        "request=makeKeyAndOrderFront".to_owned(),
        "end_active=0 end_key=0 end_frontmost=0".to_owned(),
    ];
    assert!(activation_watch::classify(&passing, true).is_ok());
    assert!(activation_watch::classify(&["installed=1".to_owned()], true).is_err());
    assert!(
        activation_watch::classify(
            &[
                "installed=1".to_owned(),
                "request=activate".to_owned(),
                "request=makeKeyAndOrderFront".to_owned(),
                "outcome=becameActive".to_owned(),
                "end_active=0 end_key=0 end_frontmost=0".to_owned(),
            ],
            true
        )
        .is_err()
    );
    assert!(
        activation_watch::classify(
            &[
                "installed=1".to_owned(),
                "request=activate".to_owned(),
                "request=makeKeyAndOrderFront".to_owned(),
                "exposed=10".to_owned(),
                "end_active=0 end_key=0 end_frontmost=0".to_owned(),
            ],
            true
        )
        .is_err()
    );
}

#[test]
fn external_summary_is_forwarded_with_target_case_and_mode() {
    let raw = activation_watch::format_external_summary("4242", 1, 2, 0, 0, 1);
    assert_eq!(
        activation_watch::forwarded_external_summaries("glfw.window", "case", "typing", &raw),
        [format!(
            "activation watch external [glfw.window case typing]: {raw}"
        )]
    );
    let second = activation_watch::format_external_summary("4243", 0, 1, 0, 1, 0);
    let forwarded = activation_watch::forwarded_external_summaries(
        "glfw.window",
        "case",
        "typing",
        &format!("ordinary stdout\n{raw}\nordinary stderr\n{second}\n"),
    );
    assert_eq!(forwarded.len(), 2);
    assert!(forwarded[0].contains("[glfw.window case typing]: ck-activation-external: pid=4242"));
    assert!(forwarded[1].contains("[glfw.window case typing]: ck-activation-external: pid=4243"));
    assert!(
        activation_watch::check_fixture_forwarding("glfw.window", "case", "typing", &forwarded)
            .is_err(),
        "the fixture requires exactly one forwarded line"
    );
    let fixture =
        activation_watch::forwarded_external_summaries("glfw.window", "case", "typing", &raw);
    assert!(
        activation_watch::check_fixture_forwarding("glfw.window", "case", "typing", &fixture)
            .is_ok()
    );
    assert!(
        activation_watch::forwarded_external_summaries(
            "glfw.window",
            "case",
            "quiet",
            "noise\nnot-the-fixed-prefix: pid=5\n"
        )
        .is_empty()
    );
}

#[test]
fn probe_not_run_notice_is_forwarded_with_target_case_and_mode() {
    let raw = "ck-probe-not-run: reason=isOnActiveSpace probe=71 probe_pid=42 under=72 under_pid=42 controls=0:71:42,1:0:-1";
    assert_eq!(
        activation_watch::forwarded_external_summaries(
            "glfw.window",
            "test_windows_let_clicks_through",
            "quiet",
            &format!("ordinary output\n{raw}\n")
        ),
        [format!(
            "pass-through probe not run [glfw.window test_windows_let_clicks_through quiet]: {raw}"
        )]
    );
}
