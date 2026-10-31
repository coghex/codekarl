//! The Vulkan library's tests that open no window: the loader, the instance (headless, as the readback
//! uses it), the off-screen readback and its ownership when its completion is unproven, the loader's
//! lifetime and validation from the creation of an instance to its destruction. Validation is on in every
//! case, and any validation message fails it. A missing loader or validation layer fails the test (nothing
//! is skipped).

use std::env;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ck_gpu_vulkan::{
    Instance, InstanceOptions, InstanceTestHooks, Loader, ReadbackHooks, ValidationLogHandle,
    create_instance, create_instance_with, create_instance_with_test_hooks,
    destroy_surface_and_instance, load_loader, load_loader_from, readback_clear,
    readback_clear_with_test_hooks, search_directories, vk,
};

#[path = "../../../apps/codekarl/tests/supervisor/mod.rs"]
#[expect(
    dead_code,
    reason = "the shared supervisor serves other tests; this one uses its spawn and finish"
)]
mod supervisor;

use supervisor::Supervised;

mod gpu_bindless;
mod gpu_quad;
mod gpu_scene;
mod gpu_textures;

/// A panic payload whose `Drop` panics (and counts itself): the caught payload's disposal must survive it.
struct Bomb(&'static AtomicUsize);

impl Drop for Bomb {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("a payload whose drop panics");
    }
}

static BOMBS_BEFORE_SUBMIT: AtomicUsize = AtomicUsize::new(0);
static BOMBS_AFTER_SUBMIT: AtomicUsize = AtomicUsize::new(0);

fn bomb_before_submit() -> ! {
    std::panic::panic_any(Bomb(&BOMBS_BEFORE_SUBMIT))
}

fn bomb_after_submit() -> ! {
    std::panic::panic_any(Bomb(&BOMBS_AFTER_SUBMIT))
}

/// `1` makes a child send one warning through its instance's own messenger right after the instance is
/// created: every case must then fail.
const INJECT_VARIABLE: &str = "CK_VULKAN_TEST_INJECT_VALIDATION";

/// A validated instance with **no** window extensions: headless use, as the readback has.
fn validated(loader: &Loader) -> Instance {
    validated_with(loader, false)
}

/// [`validated`], with synchronization validation too when `synchronization` is set.
fn validated_with(loader: &Loader, synchronization: bool) -> Instance {
    let instance = create_instance_with(
        loader,
        &[],
        InstanceOptions {
            validation: true,
            synchronization,
        },
    )
    .expect(
        "a validated headless instance (the loader, the layer and the extensions are installed)",
    );
    if env::var(INJECT_VARIABLE).as_deref() == Ok("1") {
        assert!(
            instance.submit_test_warning("an injected validation message"),
            "the instance has a messenger"
        );
    }
    instance
}

/// Validation said nothing, from the creation of the instance to this moment.
fn assert_log_clean(log: &ValidationLogHandle) {
    assert_eq!(
        log.messages(),
        Vec::<String>::new(),
        "validation reported something"
    );
}

/// Destroys an instance that has no surface and no presenter.
fn destroy(instance: Instance) {
    // SAFETY: there is no surface (null) and no presenter ever used the instance.
    let outcome = unsafe { destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None) };
    if let Err(refused) = outcome {
        panic!("the instance was not destroyed: {refused}");
    }
}

/// 1. Readback: a known colour reads back exactly (within one unit) under validation, with no window.
#[test]
fn a_known_colour_reads_back_exactly_under_validation() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    for rgb in [
        [0.0_f32, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [1.0, 0.5, 0.25],
        [0.2, 0.4, 0.6],
        [0.1, 0.9, 0.3],
    ] {
        let pixel = readback_clear(&instance, rgb).expect("the readback works");
        for (channel, wanted) in rgb.iter().enumerate() {
            let expected = (wanted * 255.0).round();
            assert!(
                (f32::from(pixel[channel]) - expected).abs() <= 1.0,
                "{rgb:?} read back as {pixel:?}"
            );
        }
        assert_eq!(pixel[3], 255, "alpha is one");
    }
    assert_log_clean(&log);
    assert_eq!(
        instance.retained_readbacks(),
        0,
        "a proven readback retains nothing"
    );
    destroy(instance);
    assert_log_clean(&log);
}

/// 2 and 14. The instance: portability enumeration where the implementation is one, the present-fence
/// extensions and **their dependencies whatever the caller passes** (an empty list is headless use and a
/// valid instance, with no validation message), and a missing extension is an error naming it.
#[test]
fn the_instance_enables_what_it_needs_and_names_what_is_missing() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let extensions = instance.enabled_extensions().to_vec();
    for needed in [
        "VK_KHR_surface",
        "VK_EXT_surface_maintenance1",
        "VK_KHR_get_surface_capabilities2",
        "VK_EXT_debug_utils",
    ] {
        assert!(
            extensions.iter().any(|name| name == needed),
            "{needed} in {extensions:?}"
        );
    }
    assert_eq!(
        extensions
            .iter()
            .any(|name| name == "VK_KHR_portability_enumeration"),
        instance.is_portability(),
        "portability enumeration is enabled exactly for a portability implementation"
    );
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
    let missing = create_instance(&loader, &["VK_EXT_no_such_extension".to_owned()])
        .expect_err("a missing extension is an error");
    assert!(
        missing.message().contains("VK_EXT_no_such_extension"),
        "the error names the extension: {missing}"
    );
}

/// 14. A headless instance without validation is valid too (an empty extension list).
#[test]
fn a_headless_instance_needs_no_extension_from_the_caller() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let plain = create_instance(&loader, &[]).expect("an instance with no required extensions");
    assert!(
        plain
            .enabled_extensions()
            .iter()
            .any(|name| name == "VK_KHR_surface"),
        "{:?}",
        plain.enabled_extensions()
    );
    destroy(plain);
}

/// The validation path records what the layer says: a message sent through the instance's own messenger is
/// in the log (so "no validation messages" means something).
#[test]
fn validation_messages_are_recorded() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = create_instance_with(
        &loader,
        &[],
        InstanceOptions {
            validation: true,
            ..InstanceOptions::default()
        },
    )
    .expect("a validated instance");
    assert!(instance.validation_messages().is_empty());
    assert!(instance.submit_test_warning("a test warning"));
    let messages = instance.validation_messages();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("a test warning"), "{messages:?}");
    let plain = create_instance(&loader, &[]).expect("an instance without validation");
    assert!(
        !plain.submit_test_warning("nobody listens"),
        "no messenger without validation"
    );
    destroy(plain);
    destroy(instance);
}

/// Test 16: a diagnostic of the instance's creation is recorded: the instance is made invalid on purpose (the
/// extension dependencies omitted) and the layer's errors from `vkCreateInstance` are in the log. Run in a
/// child process of its own: it provokes invalid use, which must not disturb the other tests' Vulkan state.
#[test]
fn a_creation_time_diagnostic_is_recorded() {
    let seen = run_child("child_a_creation_time_diagnostic", false);
    assert_eq!(seen.0, Some(0), "the child failed:\n{}", seen.1);
    assert!(
        seen.1.contains("1 passed"),
        "the child ran its test:\n{}",
        seen.1
    );
}

#[test]
#[ignore = "run by a_creation_time_diagnostic_is_recorded"]
fn child_a_creation_time_diagnostic() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = create_instance_with_test_hooks(
        &loader,
        &[],
        InstanceOptions {
            validation: true,
            ..InstanceOptions::default()
        },
        InstanceTestHooks {
            omit_dependencies: true,
        },
    )
    .expect("the instance is created, invalid as asked");
    let messages = instance.validation_messages();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("VUID-vkCreateInstance")),
        "the layer's creation-time errors are recorded: {messages:?}"
    );
    destroy(instance);
}

/// Test 16: a diagnostic of the instance's destruction is recorded: a device is leaked on purpose, and the layer
/// reports it while the instance is destroyed, into a log that outlives the instance. Run in a child process
/// of its own, for the same reason.
#[test]
fn a_destruction_time_diagnostic_is_recorded() {
    let seen = run_child("child_a_destruction_time_diagnostic", false);
    assert_eq!(seen.0, Some(0), "the child failed:\n{}", seen.1);
    assert!(
        seen.1.contains("1 passed"),
        "the child ran its test:\n{}",
        seen.1
    );
}

#[test]
#[ignore = "run by a_destruction_time_diagnostic_is_recorded"]
fn child_a_destruction_time_diagnostic() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = create_instance_with(
        &loader,
        &[],
        InstanceOptions {
            validation: true,
            ..InstanceOptions::default()
        },
    )
    .expect("a validated instance");
    let log = instance.validation_log();
    assert_log_clean(&log);
    assert!(instance.leak_a_device_for_tests());
    destroy(instance);
    let messages = log.messages();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("vkDestroyInstance")
                || message.contains("VUID-vkDestroyInstance")),
        "the layer's destruction-time errors are recorded: {messages:?}"
    );
}

/// The loader is found by an explicit, documented search and loaded by absolute path.
#[test]
fn the_loader_is_loaded_by_an_absolute_path_and_a_missing_one_lists_the_places_searched() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let path = loader
        .path()
        .expect("found in a listed directory, not by the default search");
    assert!(path.is_absolute(), "{path:?}");
    assert!(
        search_directories()
            .iter()
            .any(|directory| path.starts_with(directory)),
        "{path:?} is in one of {:?}",
        search_directories()
    );
    assert!(loader.instance_version().expect("a version") >= vk::API_VERSION_1_1);
    let nowhere = PathBuf::from("/nonexistent/vulkan/lib");
    let error = load_loader_from(std::slice::from_ref(&nowhere), false)
        .expect_err("nothing there and no default search");
    assert!(
        error.message().contains("/nonexistent/vulkan/lib"),
        "{error}"
    );
    assert!(error.message().contains("not there"), "{error}");
    // With the default search as a last resort, the error also says it was tried (or it loads).
    match load_loader_from(&[nowhere], true) {
        Ok(found) => assert!(
            found.path().is_none(),
            "found by the platform's default search"
        ),
        Err(error) => assert!(error.message().contains("default search"), "{error}"),
    }
}

/// 8. Loader lifetime without a window: the library is released only after the last clone, every instance
///    and every readback using it are gone.
#[test]
fn the_library_is_released_only_after_every_user_is_gone() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    assert!(loader.clones() >= 2, "the instance holds its own clone");
    // The test's own clone goes first: the instance keeps the library loaded, and the readback works.
    drop(loader);
    assert!(!watch.released(), "the instance still uses the library");
    let pixel = readback_clear(&instance, [0.5, 0.5, 0.5])
        .expect("a readback with only the instance's clone");
    assert!((i32::from(pixel[0]) - 128).abs() <= 1, "{pixel:?}");
    let held = instance.loader().clone();
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
    assert!(!watch.released(), "this test's own clone is still held");
    assert_eq!(held.clones(), 1);
    drop(held);
    assert!(watch.released(), "the last drop unloads the library");
}

/// 15. An unproven readback keeps its instance and its loader clone: nothing it owns is released by the
///     caller dropping its own handles, and the instance cannot be destroyed under it.
fn unproven_readback_keeps_its_owners(name: &str, hooks: ReadbackHooks) {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let error = readback_clear_with_test_hooks(&instance, [1.0, 0.0, 0.0], hooks)
        .expect_err("an unproven readback is an error");
    assert!(
        error.message().contains("unproven"),
        "{name}: the error names the unproven state: {error}"
    );
    assert_eq!(instance.retained_readbacks(), 1, "{name}");
    assert_eq!(
        instance.holders(),
        base_holders + 1,
        "{name}: the instance reference is kept"
    );
    assert_eq!(
        loader.clones(),
        base_clones + 1,
        "{name}: the loader clone is kept"
    );
    assert_log_clean(&log);
    // The instance cannot be destroyed under it.
    // SAFETY: there is no surface (null); the call is refused and destroys nothing.
    let refused = unsafe { destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None) }
        .expect_err("the retained readback keeps the instance");
    assert!(
        refused.error.message().contains("readback"),
        "{name}: {refused}"
    );
    assert!(
        !refused.surface_destroyed,
        "{name}: there was no surface to destroy"
    );
    // Validation is asserted again after the refused destruction, still without destroying what is retained.
    assert_log_clean(&log);
    // The caller drops everything it owns: the library stays loaded.
    drop(refused.instance);
    drop(loader);
    assert!(
        !watch.released(),
        "{name}: the retained readback keeps the library loaded"
    );
}

#[test]
fn a_readback_whose_fence_wait_times_out_keeps_its_owners() {
    unproven_readback_keeps_its_owners(
        "wait timeout",
        ReadbackHooks {
            wait: Some(vk::Result::TIMEOUT),
            ..ReadbackHooks::default()
        },
    );
}

#[test]
fn a_readback_whose_fence_wait_loses_the_device_keeps_its_owners() {
    unproven_readback_keeps_its_owners(
        "wait device lost",
        ReadbackHooks {
            wait: Some(vk::Result::ERROR_DEVICE_LOST),
            ..ReadbackHooks::default()
        },
    );
}

#[test]
fn a_readback_whose_submit_loses_the_device_keeps_its_owners() {
    unproven_readback_keeps_its_owners(
        "submit device lost",
        ReadbackHooks {
            submit: Some(vk::Result::ERROR_DEVICE_LOST),
            ..ReadbackHooks::default()
        },
    );
}

/// An unwind after the submit is caught at the readback boundary and **returned as an `Error`** naming the
/// unproven state; nothing is released. The test receives the `Result`, with no panic handler of its own.
#[test]
fn a_readback_that_unwinds_after_its_submit_returns_an_error_and_keeps_its_owners() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let error = readback_clear_with_test_hooks(
        &instance,
        [0.0, 1.0, 0.0],
        ReadbackHooks {
            panic_after_submit: true,
            ..ReadbackHooks::default()
        },
    )
    .expect_err("the unwind is an error, not a panic");
    assert!(
        error.message().contains("unwound after its submit")
            && error.message().contains("unproven"),
        "{error}"
    );
    assert_eq!(instance.retained_readbacks(), 1);
    assert_eq!(
        instance.holders(),
        base_holders + 1,
        "the unwind kept the instance reference"
    );
    assert_eq!(
        loader.clones(),
        base_clones + 1,
        "the unwind kept the loader clone"
    );
    // Validation is asserted on this retained exit, without destroying what is retained.
    assert_log_clean(&log);
    drop(instance);
    drop(loader);
    assert!(!watch.released(), "the library stays loaded");
}

/// An unwind before the submit has nothing in flight: it cleans up and returns an `Error`.
#[test]
fn a_readback_that_unwinds_before_its_submit_cleans_up() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let error = readback_clear_with_test_hooks(
        &instance,
        [0.0, 1.0, 0.0],
        ReadbackHooks {
            panic_before_submit: true,
            ..ReadbackHooks::default()
        },
    )
    .expect_err("the unwind is an error");
    assert!(
        error.message().contains("unwound before its submit"),
        "{error}"
    );
    assert!(!error.message().contains("unproven"), "{error}");
    assert_eq!(instance.retained_readbacks(), 0);
    assert_eq!(instance.holders(), base_holders);
    assert_eq!(loader.clones(), base_clones);
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// A panic whose payload's own `Drop` panics, before the submit, is caught and disposed of at the readback
/// boundary: the same clean error as an ordinary payload, every object destroyed, the holds released. The payload
/// is dropped (once) inside the disposal, and nothing escapes the readback.
#[test]
fn a_payload_whose_drop_panics_before_the_submit_is_the_same_clean_error() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        readback_clear_with_test_hooks(
            &instance,
            [0.0, 1.0, 0.0],
            ReadbackHooks {
                panic_before_submit: true,
                forced_panic_payload: Some(bomb_before_submit),
                ..ReadbackHooks::default()
            },
        )
    }));
    let Ok(result) = outcome else {
        panic!(
            "the payload's drop panic escaped the readback (retained readbacks: {})",
            instance.retained_readbacks()
        );
    };
    let error = result.expect_err("the unwind is an error");
    assert!(
        error.message().contains("unwound before its submit")
            && error
                .message()
                .contains("a panic with a non-string payload"),
        "{error}"
    );
    assert!(!error.message().contains("unproven"), "{error}");
    assert_eq!(
        BOMBS_BEFORE_SUBMIT.load(Ordering::SeqCst),
        1,
        "disposed once"
    );
    assert_eq!(instance.retained_readbacks(), 0);
    assert_eq!(instance.holders(), base_holders);
    assert_eq!(loader.clones(), base_clones);
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// The same payload after the submit: the unproven error, with the owners kept as the plain-payload test above
/// checks.
#[test]
fn a_payload_whose_drop_panics_after_the_submit_is_the_same_unproven_error() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let watch = loader.watch();
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        readback_clear_with_test_hooks(
            &instance,
            [0.0, 1.0, 0.0],
            ReadbackHooks {
                panic_after_submit: true,
                forced_panic_payload: Some(bomb_after_submit),
                ..ReadbackHooks::default()
            },
        )
    }));
    let Ok(result) = outcome else {
        panic!(
            "the payload's drop panic escaped the readback (retained readbacks: {})",
            instance.retained_readbacks()
        );
    };
    let error = result.expect_err("the unwind is an error");
    assert!(
        error.message().contains("unwound after its submit")
            && error.message().contains("unproven")
            && error
                .message()
                .contains("a panic with a non-string payload"),
        "{error}"
    );
    assert_eq!(
        BOMBS_AFTER_SUBMIT.load(Ordering::SeqCst),
        1,
        "disposed once"
    );
    assert_eq!(instance.retained_readbacks(), 1);
    assert_eq!(instance.holders(), base_holders + 1);
    assert_eq!(loader.clones(), base_clones + 1);
    assert_log_clean(&log);
    drop(instance);
    drop(loader);
    assert!(!watch.released(), "the library stays loaded");
}

/// A submit that queued nothing (out of host or device memory) cleans up and is an ordinary error.
#[test]
fn a_readback_whose_submit_ran_out_of_memory_cleans_up() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let base_clones = loader.clones();
    let base_holders = instance.holders();
    for code in [
        vk::Result::ERROR_OUT_OF_HOST_MEMORY,
        vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
    ] {
        let error = readback_clear_with_test_hooks(
            &instance,
            [0.3, 0.3, 0.3],
            ReadbackHooks {
                submit: Some(code),
                ..ReadbackHooks::default()
            },
        )
        .expect_err("the submit failed");
        assert!(error.message().contains("vkQueueSubmit"), "{error}");
        assert!(!error.message().contains("unproven"), "{error}");
        assert_eq!(
            instance.retained_readbacks(),
            0,
            "{code:?}: nothing retained"
        );
        assert_eq!(instance.holders(), base_holders, "{code:?}");
        assert_eq!(loader.clones(), base_clones, "{code:?}");
    }
    // A readback still works afterwards, and the instance can be destroyed.
    let pixel = readback_clear(&instance, [1.0, 1.0, 1.0]).expect("a readback after the failures");
    assert_eq!(pixel, [255, 255, 255, 255]);
    assert_log_clean(&log);
    destroy(instance);
    assert_log_clean(&log);
}

/// 8. The no-loader-variable path: a child process with no `DYLD_*` variable (Cargo sets one for this
///    test) loads the loader by its absolute path, creates an instance and reads back.
#[test]
fn a_process_without_any_dynamic_loader_variable_loads_and_uses_the_loader() {
    let seen = run_child("child_uses_the_loader_without_variables", false);
    assert_eq!(seen.0, Some(0), "the child failed:\n{}", seen.1);
    assert!(
        seen.1.contains("1 passed"),
        "the child ran its test:\n{}",
        seen.1
    );
}

/// Runs one ignored test of this executable as a supervised child, with every dynamic-loader variable
/// removed; `inject` sends a validation warning in the child.
fn run_child(test: &str, inject: bool) -> (Option<i32>, String) {
    run_test(test, inject, true)
}

/// Runs one test of this executable as a supervised child (`ignored` for the child-only tests, which the
/// parent tests start), with every dynamic-loader variable removed.
fn run_test(test: &str, inject: bool, ignored: bool) -> (Option<i32>, String) {
    let mut command = Command::new(env::current_exe().expect("the test executable"));
    if ignored {
        command.arg("--ignored");
    }
    command.args(["--exact", test, "--nocapture"]);
    for (name, _) in env::vars() {
        if name.starts_with("DYLD_") || name == "LD_LIBRARY_PATH" {
            command.env_remove(name);
        }
    }
    command.env_remove(INJECT_VARIABLE);
    if inject {
        command.env(INJECT_VARIABLE, "1");
    }
    let seen = Supervised::spawn(command, Duration::from_secs(60))
        .and_then(Supervised::finish)
        .expect("the child ran under the supervisor");
    (seen.code, seen.stdout)
}

/// Run only by the test above, in a process with no dynamic-loader variable.
#[test]
#[ignore = "run by a_process_without_any_dynamic_loader_variable_loads_and_uses_the_loader"]
fn child_uses_the_loader_without_variables() {
    assert!(
        env::vars().all(|(name, _)| !name.starts_with("DYLD_") && name != "LD_LIBRARY_PATH"),
        "the child has no dynamic-loader variable"
    );
    let loader = load_loader().expect("the Vulkan loader loads by its absolute path");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let pixel = readback_clear(&instance, [1.0, 0.0, 0.0]).expect("a readback");
    assert_eq!(pixel, [255, 0, 0, 255]);
    assert_log_clean(&log);
    destroy(instance);
}

/// 16. Validation reaches every kind of case: with a warning injected into the instance (through its own
///     messenger), a case that ends proven fails, and so does one that ends with a retained readback.
#[test]
fn an_injected_validation_message_fails_a_proven_case_and_a_retained_one() {
    for child in [
        "child_a_proven_case_asserts_the_log",
        "child_a_retained_readback_case_asserts_the_log",
    ] {
        let clean = run_child(child, false);
        assert_eq!(
            clean.0,
            Some(0),
            "{child} passes without an injection:\n{}",
            clean.1
        );
        let injected = run_child(child, true);
        assert_ne!(
            injected.0,
            Some(0),
            "{child} must fail with an injected message"
        );
        assert!(
            injected.1.contains("validation reported"),
            "{child} fails for the validation message:\n{}",
            injected.1
        );
    }
}

/// 16. Every real test that ends with a validated instance fails when a warning is injected into it: this
///     runs the actual tests, so a case that does not assert the log (as the unwinding readback case did)
///     is caught here.
#[test]
fn an_injected_validation_message_fails_every_case_that_uses_a_validated_instance() {
    for test in [
        "a_known_colour_reads_back_exactly_under_validation",
        "the_instance_enables_what_it_needs_and_names_what_is_missing",
        "the_library_is_released_only_after_every_user_is_gone",
        "a_readback_whose_fence_wait_times_out_keeps_its_owners",
        "a_readback_whose_fence_wait_loses_the_device_keeps_its_owners",
        "a_readback_whose_submit_loses_the_device_keeps_its_owners",
        "a_readback_that_unwinds_after_its_submit_returns_an_error_and_keeps_its_owners",
        "a_readback_that_unwinds_before_its_submit_cleans_up",
        "a_readback_whose_submit_ran_out_of_memory_cleans_up",
    ] {
        let clean = run_test(test, false, false);
        assert_eq!(
            clean.0,
            Some(0),
            "{test} passes without an injection:\n{}",
            clean.1
        );
        let injected = run_test(test, true, false);
        assert_ne!(
            injected.0,
            Some(0),
            "{test} must fail with an injected message"
        );
        assert!(
            injected.1.contains("validation reported"),
            "{test} fails for the validation message:\n{}",
            injected.1
        );
    }
}

#[test]
#[ignore = "run by an_injected_validation_message_fails_a_proven_case_and_a_retained_one"]
fn child_a_proven_case_asserts_the_log() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    readback_clear(&instance, [0.5, 0.5, 0.5]).expect("a readback");
    destroy(instance);
    assert_log_clean(&log);
}

#[test]
#[ignore = "run by an_injected_validation_message_fails_a_proven_case_and_a_retained_one"]
fn child_a_retained_readback_case_asserts_the_log() {
    let loader = load_loader().expect("the Vulkan loader loads");
    let instance = validated(&loader);
    let log = instance.validation_log();
    let _ = readback_clear_with_test_hooks(
        &instance,
        [0.5, 0.5, 0.5],
        ReadbackHooks {
            wait: Some(vk::Result::TIMEOUT),
            ..ReadbackHooks::default()
        },
    );
    // Retained: nothing is destroyed, and the log is asserted all the same.
    assert_eq!(instance.retained_readbacks(), 1);
    assert_log_clean(&log);
}
