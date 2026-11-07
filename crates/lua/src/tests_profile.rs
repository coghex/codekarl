//! Deterministic-profile contracts for the gameplay VM.

use super::*;
use crate::Profile;

#[test]
fn deterministic_profile_removes_host_inputs_orders_pairs_and_hides_addresses() {
    let dir = Dir::new();
    let entry = dir.script(
        r"
assert(os == nil and io == nil and package == nil and require == nil)
assert(loadfile == nil and dofile == nil and collectgarbage == nil and next == nil)
assert(math.random == nil and math.randomseed == nil and string.dump == nil)
local t = {[2] = 'two', [1] = 'one', z = true, a = true}
local order = {}
for key in pairs(t) do order[#order + 1] = tostring(key) end
assert(table.concat(order, ',') == '1,2,a,z')
local value = {}
assert(tostring(value) == 'table')
assert(string.format('%s', value) == 'table')
assert(string.format('%10s', value) == '     table')
assert(not pcall(string.format, '%p', value))
local protected = setmetatable({}, {__tostring = function() return 'custom' end, __metatable = 'locked'})
assert(tostring(protected) == 'custom')
assert(string.format('%s', protected) == 'custom')
local function callback() end
local thread = coroutine.create(function() end)
assert(tostring(callback) == 'function' and tostring(thread) == 'thread')
assert(string.format('%s/%s', callback, thread) == 'function/thread')
print(callback, thread)
    log.info(callback)
    log.info(thread)
",
    );
    let mut settings = VmSettings::new("deterministic", entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("the deterministic main chunk runs");
    assert_eq!(records("deterministic"), ["INFO function", "INFO thread"]);
    owner.stop(BOUND).expect("the owner stops");
}

#[test]
fn locale_mutation_is_removed_before_user_code_in_both_profiles() {
    for (name, profile, source) in [
        (
            "standard_locale_guard",
            Profile::Standard,
            "assert(os ~= nil and os.time ~= nil and os.setlocale == nil)",
        ),
        (
            "deterministic_locale_guard",
            Profile::Deterministic,
            "assert(os == nil)",
        ),
    ] {
        let dir = Dir::new();
        let entry = dir.script(source);
        let mut settings = VmSettings::new(name, entry, empty_config());
        settings.profile = profile;
        let mut owner = VmOwner::spawn(settings).expect("the thread starts");
        owner
            .wait_ready(BOUND)
            .expect("locale guard runs before entry");
        owner.stop(BOUND).expect("the owner stops");
    }
}

#[test]
fn deterministic_pairs_and_sort_follow_byte_order_and_stable_merge_rules() {
    let dir = Dir::new();
    let entry = dir.script(
        r#"
local keyed = {['a'] = true, ['B'] = true, ['_'] = true, ['ab'] = true,
  ['a\0'] = true, ['\xc3\xa9'] = true, ['Z'] = true}
local keys = {}
for key in pairs(keyed) do keys[#keys + 1] = key end
assert(table.concat(keys, '\0') == 'B\0Z\0_\0a\0a\0\0ab\0\xc3\xa9')

local numbers = {}
for i = 300, 1, -1 do numbers[#numbers + 1] = i end
table.sort(numbers)
for i = 1, 300 do assert(numbers[i] == i) end

local strings = {}
for i = 300, 1, -1 do strings[#strings + 1] = string.char((i % 250) + 1) end
table.sort(strings)
for i = 2, #strings do assert(not (strings[i] < strings[i - 1])) end
local high = {'\xc3\xa9', 'Z', 'a\0', 'ab', '_', 'B', 'a'}
table.sort(high)
assert(table.concat(high, '|') == 'B|Z|_|a|a\0|ab|\xc3\xa9')

local records = {}
for i = 1, 300 do records[i] = {key = i % 3, id = i} end
table.sort(records, function(a, b) return a.key < b.key end)
local previous = {-1, -1, -1}
for i = 1, #records do
  local item = records[i]
  assert(item.id > previous[item.key + 1])
  previous[item.key + 1] = item.id
end

local raw = {5, 1, 4, 2, 3}
local proxy = setmetatable({}, {
  __len = function() return 5 end,
  __index = function(_, index) return raw[index] end,
  __newindex = function(_, index, value) raw[index] = value end
})
assert(select('#', table.sort(proxy)) == 0)
assert(table.concat(raw, ',') == '1,2,3,4,5')
local ok_mixed, mixed_message = pcall(table.sort, {1, 'one'})
assert(not ok_mixed and mixed_message:find('attempt to compare', 1, true))
local ok_table, table_message = pcall(table.sort, 5)
assert(not ok_table and table_message:find("bad argument #1 to 'sort' (table expected", 1, true))
local ok_comp, comp_message = pcall(table.sort, {}, 5)
assert(not ok_comp and comp_message:find("bad argument #2 to 'sort' (function expected", 1, true))
local ok_error, error_message = pcall(table.sort, {3, 2, 1}, function() error('comparator boom') end)
assert(not ok_error and error_message:find('comparator boom', 1, true))
"#,
    );
    let mut settings = VmSettings::new("deterministic_sort_contract", entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("byte order and stable sort hold");
    owner.stop(BOUND).expect("the owner stops");
}

fn inconsistent_sort_result(name: &str, comparator: &str) -> String {
    let source = format!(
        "local t = {{}}; for i = 1, 300 do t[i] = i end; local calls = 0; \
         table.sort(t, function(a, b) calls = calls + 1; {comparator} end); \
         log.info(calls .. ':' .. table.concat(t, ','))"
    );
    let dir = Dir::new();
    let entry = dir.script(&source);
    let mut settings = VmSettings::new(name, entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("inconsistent comparison returns");
    let result = records(name).last().expect("the result was logged").clone();
    owner.stop(BOUND).expect("the owner stops");
    result
}

#[test]
fn deterministic_sort_inconsistent_comparators_have_repeatable_permutations() {
    for comparator in ["return true", "return calls % 2 == 0"] {
        let first = inconsistent_sort_result("sort_inconsistent_a", comparator);
        let second = inconsistent_sort_result("sort_inconsistent_b", comparator);
        assert_eq!(first, second);
    }
}

#[test]
fn standard_profile_keeps_native_inconsistent_sort_error() {
    let dir = Dir::new();
    let entry = dir.script(
        "local t = {}; for i = 1, 300 do t[i] = i end; \
         local ok, message = pcall(table.sort, t, function() return true end); \
         assert(not ok and message:find('invalid order function for sorting', 1, true))",
    );
    let mut owner = VmOwner::spawn(VmSettings::new(
        "standard_native_sort",
        entry,
        empty_config(),
    ))
    .expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("native sort keeps its diagnostic");
    owner.stop(BOUND).expect("the owner stops");
}

#[test]
fn deterministic_profile_normalizes_error_objects_and_fixes_the_entry_chunk_name() {
    let mut seen = Vec::new();
    for path in ["a-machine-specific-directory.lua", "another/path/entry.lua"] {
        let dir = Dir::new();
        let entry = dir.0.join(path);
        std::fs::create_dir_all(entry.parent().unwrap()).expect("the fixture directory");
        std::fs::write(&entry, "error({})\n").expect("the fixture script");
        let mut settings = VmSettings::new("deterministic_error", entry, empty_config());
        settings.profile = Profile::Deterministic;
        let mut owner = VmOwner::spawn(settings).expect("the thread starts");
        let error = owner.wait_ready(BOUND).expect_err("the script fails");
        let (message, traceback) = script_error(&error, "deterministic_error");
        assert_eq!(message, "(error object is a table value)");
        assert!(traceback.contains("gameplay.lua:1:"), "{traceback}");
        assert!(!traceback.contains(path), "{traceback}");
        assert!(!message.contains("0x"), "{message}");
        seen.push((message, traceback));
    }
    assert_eq!(seen[0], seen[1], "entry errors do not expose the path");
}

fn posted_error(source: &str, name: &str) -> (String, String) {
    use std::thread;
    use std::time::Instant;

    let dir = Dir::new();
    let entry = dir.script(source);
    let mut settings = VmSettings::new(name, entry, empty_config());
    settings.profile = Profile::Deterministic;
    settings.call_limit = 1;
    settings.extensions = vec![crate::Extension::new("sim").callback("step", "step")];
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner.wait_ready(BOUND).expect("callback is registered");
    owner.start(BOUND).expect("the VM starts");
    let id = owner
        .post("step", Vec::new())
        .expect("the callback is posted");
    let deadline = Instant::now() + BOUND;
    let (message, traceback) = loop {
        if let Some(result) = owner.take_result() {
            assert_eq!(result.id, id);
            match result.outcome {
                crate::CallOutcome::Failed { message, traceback } => break (message, traceback),
                other => panic!("expected a callback error, got {other:?}"),
            }
        }
        assert!(Instant::now() < deadline, "the callback result arrives");
        thread::yield_now();
    };
    owner
        .stop(BOUND)
        .expect("the owner stops after a script error");
    (message, traceback)
}

#[test]
fn deterministic_error_objects_are_stable_in_callbacks_entry_and_startup() {
    let source = "sim.step(function() error({}) end)";
    let first = posted_error(source, "deterministic_callback_a");
    let second = posted_error(source, "deterministic_callback_b");
    assert_eq!(
        first, second,
        "separate VMs normalize the same error identically"
    );
    assert_eq!(first.0, "(error object is a table value)");
    assert!(first.1.contains("gameplay.lua:1:"), "{}", first.1);
    let custom_callback = posted_error(
        "sim.step(function() error(setmetatable({}, {__tostring = function() return 'boom' end})) end)",
        "deterministic_callback_tostring",
    );
    assert_eq!(custom_callback.0, "boom");

    let custom = "error(setmetatable({}, {__tostring = function() return 'boom' end}))";
    let dir = Dir::new();
    let entry = dir.script(&format!("{custom}\n"));
    let mut settings = VmSettings::new("deterministic_tostring_error_entry", entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    let error = owner.wait_ready(BOUND).expect_err("entry fails");
    assert_eq!(
        script_error(&error, "deterministic_tostring_error_entry").0,
        "boom"
    );

    let dir = Dir::new();
    let entry = dir.script(&format!("function startup() {custom} end\n"));
    let mut settings = VmSettings::new(
        "deterministic_tostring_error_startup",
        entry,
        empty_config(),
    );
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner.wait_ready(BOUND).expect("entry loads");
    owner.request_start().expect("startup is requested");
    let error = owner.await_started(BOUND).expect_err("startup fails");
    assert_eq!(
        script_error(&error, "deterministic_tostring_error_startup").0,
        "boom"
    );
}

#[test]
fn deterministic_config_pairs_uses_the_profile_key_order() {
    let dir = Dir::new();
    let entry = dir.script(
        r"
local root = {}
for key in pairs(config) do root[#root + 1] = key end
assert(table.concat(root, ',') == 'a,nested,z')
local nested = {}
for key in pairs(config.nested) do nested[#nested + 1] = key end
assert(table.concat(nested, ',') == 'first,last')
",
    );
    let config = ConfigValue::Table(
        [
            ("z".to_owned(), ConfigValue::Integer(1)),
            (
                "nested".to_owned(),
                ConfigValue::Table(
                    [
                        ("last".to_owned(), ConfigValue::Integer(2)),
                        ("first".to_owned(), ConfigValue::Integer(1)),
                    ]
                    .into(),
                ),
            ),
            ("a".to_owned(), ConfigValue::Integer(0)),
        ]
        .into(),
    );
    let mut settings = VmSettings::new("deterministic_config_pairs", entry, config);
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("config iterates in byte order");
    owner.stop(BOUND).expect("the owner stops");
}

#[test]
fn deterministic_pairs_rejects_unsupported_keys_and_reads_live_values_in_snapshot_order() {
    let dir = Dir::new();
    let entry = dir.script(
        r"
local t = {[-2] = 'negative', [3] = 'positive', ['abz'] = true, ['aba'] = true}
local order = {}
for key, value in pairs(t) do
  order[#order + 1] = tostring(key) .. ':' .. tostring(value)
  if key == -2 then t[-2] = 'changed' end
  if key == 3 then t.aba = nil end
end
assert(table.concat(order, ',') == '-2:negative,3:positive,abz:true')
local custom = setmetatable({}, {__pairs = function()
  local done = false
  return function()
    if done then return nil end
    done = true
    return 'custom', 'yes'
  end
end})
local custom_key, custom_value = pairs(custom)()
assert(custom_key == 'custom' and custom_value == 'yes')
local protected_pairs = setmetatable({}, {
  __pairs = function() return function() return 'protected', 'yes' end end,
  __metatable = 'locked'
})
local protected_key, protected_value = pairs(protected_pairs)()
assert(protected_key == 'protected' and protected_value == 'yes')
local ok, message = pcall(function() local value = {}; value[false] = 1; for _ in pairs(value) do end end)
assert(not ok and message:find('boolean', 1, true))
local ok_table, table_message = pcall(function() local value = {}; value[{}] = 1; for _ in pairs(value) do end end)
assert(not ok_table and table_message:find('table', 1, true))
local ok_float, float_message = pcall(function() local value = {[1.5] = 1}; for _ in pairs(value) do end end)
assert(not ok_float and float_message:find('1.5', 1, true))
local ok_inf, inf_message = pcall(function() local value = {[math.huge] = 1}; for _ in pairs(value) do end end)
assert(not ok_inf and inf_message:find('inf', 1, true))
local integral = {[1.0] = true}
assert(pairs(integral)() == 1)
",
    );
    let mut settings = VmSettings::new("deterministic_pairs", entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner
        .wait_ready(BOUND)
        .expect("deterministic pairs rules hold");
    owner.stop(BOUND).expect("the owner stops");
}

#[test]
fn deterministic_load_is_text_only_and_metatables_refuse_finalizers_and_weak_modes() {
    let dir = Dir::new();
    let entry = dir.script(
        r"
local chunk, text_error = load('return 42', '@text.lua', 'b')
assert(chunk and chunk() == 42, text_error)
local binary, binary_error = load('\27Lua', '@bytecode.luac', 'b')
assert(binary == nil and binary_error ~= nil)
local ok_gc, gc_error = pcall(setmetatable, {}, {__gc = function() end})
assert(not ok_gc and gc_error:find('__gc', 1, true))
local ok_mode, mode_error = pcall(setmetatable, {}, {__mode = 'k'})
assert(not ok_mode and mode_error:find('__mode', 1, true))
local indexed = setmetatable({}, {__index = {answer = 42}})
assert(indexed.answer == 42)
",
    );
    let mut settings = VmSettings::new("deterministic_load", entry, empty_config());
    settings.profile = Profile::Deterministic;
    let mut owner = VmOwner::spawn(settings).expect("the thread starts");
    owner.wait_ready(BOUND).expect("only text load is enabled");
    owner.stop(BOUND).expect("the owner stops");
}

#[test]
fn deterministic_collector_runs_after_each_posted_call() {
    use std::sync::{Arc, Condvar, Mutex, PoisonError};
    use std::time::Instant;

    for (name, allocations) in [("deterministic_gc_few", 0), ("deterministic_gc_many", 500)] {
        let dir = Dir::new();
        let source = format!(
            "for i = 1, {allocations} do local unused = {{i}} end\n{}",
            r"
local metatable = {}
local weak = setmetatable({}, metatable)
sim.step(function()
  metatable.__mode = 'k'
  local key = {}
  weak[key] = true
  assert(not sim.is_empty(weak), 'the key survives during its creating call')
end)
sim.step(function()
  assert(sim.is_empty(weak), 'the host collected after the previous call')
end)
"
        );
        let entry = dir.script(&source);
        let changed = Arc::new((Mutex::new(0_u64), Condvar::new()));
        let notify = Arc::clone(&changed);
        let mut settings = VmSettings::new(name, entry, empty_config());
        settings.profile = Profile::Deterministic;
        settings.call_limit = 1;
        settings.extensions = vec![
            crate::Extension::new("sim")
                .raw_table_is_empty("is_empty")
                .callback("step", "step"),
        ];
        settings.on_report = Some(Arc::new(move || {
            let (generation, changed) = &*notify;
            let mut generation = generation.lock().unwrap_or_else(PoisonError::into_inner);
            *generation = generation.wrapping_add(1);
            changed.notify_all();
        }));
        let mut owner = VmOwner::spawn(settings).expect("the thread starts");
        owner.wait_ready(BOUND).expect("the script loads");
        owner.start(BOUND).expect("the VM starts");
        for _ in 0..2 {
            let call = owner.post("step", Vec::new()).expect("one call fits");
            let deadline = Instant::now() + BOUND;
            loop {
                let generation = *changed.0.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(result) = owner.take_result() {
                    assert_eq!(result.id, call);
                    assert_eq!(result.outcome, crate::CallOutcome::Returned(Vec::new()));
                    break;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "posted callback report arrived");
                let guard = changed.0.lock().unwrap_or_else(PoisonError::into_inner);
                let (guard, _) = changed
                    .1
                    .wait_timeout_while(guard, left, |current| *current == generation)
                    .unwrap_or_else(PoisonError::into_inner);
                drop(guard);
            }
        }
        owner.stop(BOUND).expect("the owner stops");
    }
}
