//! Windowless regression coverage for the windowed tests' output-stream assertions.

#[path = "support/ui_pointer_streams.rs"]
mod ui_pointer_streams;

const LUA_STDOUT: &str = "INFO  lua.ui: ui-hit-published\nINFO  lua.ui: ui-pointer down fixture_panel\nINFO  lua.ui: ui-pointer up fixture_panel\nINFO  window: window closed\n";
// These lines are copied from the successful-behaviour capture in ck207-planned-check-final11.log.
const SIDECAR: &str = "camera 0 782598144 782598144\nui-click-injected\nui-click-delivered\nmap-drag-dispatched\npointer down\npointer move\npointer up\ncamera 0 782565376 782630912\n";
const VULKAN_SIDECAR: &str = "camera 0 782598144 782598144\nui-cursor-move-injected\nui-click-injected\nui-click-delivered\nui-drag-cursor-move-injected\nmap-drag-dispatched\npointer down\npointer move\npointer up\ncamera 0 782565376 782630912\n";
const INITIAL_CAMERA: &str = "camera 0 782598144 782598144\n";
const EXPECTED_VULKAN_COMBINED: &str = concat!(
    "INFO  lua.ui: ui-hit-published\n",
    "INFO  lua.ui: ui-pointer down fixture_panel\n",
    "INFO  lua.ui: ui-pointer up fixture_panel\n",
    "INFO  window: window closed\n",
    "camera 0 782598144 782598144\n",
    "ui-cursor-move-injected\n",
    "ui-click-injected\n",
    "ui-click-delivered\n",
    "ui-drag-cursor-move-injected\n",
    "map-drag-dispatched\n",
    "pointer down\n",
    "pointer move\n",
    "pointer up\n",
    "camera 0 782565376 782630912\n",
);
// Raw combined stdout captured by the failed planned check (quiet and typing). These are passed
// through the same split and full assertion as the live Vulkan window case.
const CAPTURED_VULKAN_QUIET: &str = concat!(
    "INFO  runtime: codekarl 0.1.0 starting\n",
    "INFO  config: loaded /var/folders/xs/kyf0vrg92c340wk3jncyp1fr0000gn/T/codekarl-runtime-test-8187-11/config.toml\n",
    "INFO  runtime: features: metal,vulkan\n",
    "INFO  runtime: window: glfw-vulkan\n",
    "INFO  runtime: started resources\n",
    "INFO  runtime: started gameplay\n",
    "INFO  runtime: started ui\n",
    "INFO  runtime: start-up complete\n",
    "INFO  render: vulkan loader notice (not a validation message): [WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"\n",
    "INFO  render: vulkan loader notice (not a validation message): [WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"\n",
    "INFO  render: bindless table of 4096 slots (vkGetDescriptorSetLayoutSupport: not supported although every update-after-bind limit passes; MoltenVK #2573, proceeding)\n",
    "INFO  window: window opened\n",
    "INFO  lua.ui: ui-hit-published\n",
    "INFO  lua.ui: ui-pointer move fixture_panel\n",
    "INFO  lua.ui: ui-pointer down fixture_panel\n",
    "INFO  lua.ui: ui-pointer up fixture_panel\n",
    "INFO  lua.ui: ui-pointer down fixture_panel\n",
    "INFO  lua.ui: ui-pointer move fixture_panel\n",
    "INFO  lua.ui: ui-pointer up fixture_panel\n",
    "INFO  window: window closed\n",
    "INFO  lua.ui: ui shutdown\n",
    "INFO  lua.gameplay: gameplay shutdown\n",
    "INFO  runtime: stopped ui\n",
    "INFO  runtime: stopped gameplay\n",
    "INFO  runtime: stopped resources\n",
    "INFO  runtime: shutdown complete\n",
    "camera 0 782598144 782598144\n",
    "ui-cursor-move-injected\n",
    "ui-click-injected\n",
    "ui-click-delivered\n",
    "map-drag-dispatched\n",
);
const CAPTURED_VULKAN_TYPING: &str = concat!(
    "INFO  runtime: codekarl 0.1.0 starting\n",
    "INFO  config: loaded /var/folders/xs/kyf0vrg92c340wk3jncyp1fr0000gn/T/codekarl-runtime-test-8187-67/config.toml\n",
    "INFO  runtime: features: metal,vulkan\n",
    "INFO  runtime: window: glfw-vulkan\n",
    "INFO  runtime: started resources\n",
    "INFO  runtime: started gameplay\n",
    "INFO  runtime: started ui\n",
    "INFO  runtime: start-up complete\n",
    "INFO  render: vulkan loader notice (not a validation message): [WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"\n",
    "INFO  render: vulkan loader notice (not a validation message): [WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers \"VK_LAYER_KHRONOS_validation\"\n",
    "INFO  render: bindless table of 4096 slots (vkGetDescriptorSetLayoutSupport: not supported although every update-after-bind limit passes; MoltenVK #2573, proceeding)\n",
    "INFO  window: window opened\n",
    "INFO  lua.ui: ui-hit-published\n",
    "INFO  lua.ui: ui-pointer move fixture_panel\n",
    "INFO  lua.ui: ui-pointer down fixture_panel\n",
    "INFO  lua.ui: ui-pointer up fixture_panel\n",
    "INFO  lua.ui: ui-pointer down fixture_panel\n",
    "INFO  lua.ui: ui-pointer move fixture_panel\n",
    "INFO  lua.ui: ui-pointer up fixture_panel\n",
    "INFO  window: window closed\n",
    "INFO  lua.ui: ui shutdown\n",
    "INFO  lua.gameplay: gameplay shutdown\n",
    "INFO  runtime: stopped ui\n",
    "INFO  runtime: stopped gameplay\n",
    "INFO  runtime: stopped resources\n",
    "INFO  runtime: shutdown complete\n",
    "camera 0 782598144 782598144\n",
    "ui-cursor-move-injected\n",
    "ui-click-injected\n",
    "ui-click-delivered\n",
    "map-drag-dispatched\n",
);

#[test]
fn captured_native_and_vulkan_streams_pass_and_click_camera_mutation_fails() {
    let lua_stdout = format!("{LUA_STDOUT}{INITIAL_CAMERA}");
    assert!(ui_pointer_streams::validate(&lua_stdout, SIDECAR, "native").is_ok());
    assert!(ui_pointer_streams::validate(&lua_stdout, VULKAN_SIDECAR, "Vulkan").is_ok());
    ui_pointer_streams::validate_combined_output(EXPECTED_VULKAN_COMBINED, "Vulkan")
        .unwrap_or_else(|error| panic!("{error}"));

    let doctored = VULKAN_SIDECAR.replace("ui-click-delivered", "camera 0 1 1\nui-click-delivered");
    let error = ui_pointer_streams::validate(&lua_stdout, &doctored, "Vulkan")
        .expect_err("a camera change between click injection and delivery must fail");
    assert!(error.contains("UI click moved the map"), "{error}");

    let missing_move = VULKAN_SIDECAR.replace("ui-cursor-move-injected\n", "");
    let error = ui_pointer_streams::validate(&lua_stdout, &missing_move, "Vulkan")
        .expect_err("Vulkan must move the cursor before clicking");
    assert!(error.contains("ui-cursor-move-injected"), "{error}");
}

#[test]
fn captured_vulkan_outputs_use_the_live_split_and_reject_missing_drag_cursor_moves() {
    for captured in [CAPTURED_VULKAN_QUIET, CAPTURED_VULKAN_TYPING] {
        // Both real raw captures predate the explicit move that starts the Vulkan map drag.
        let full = ui_pointer_streams::validate_combined_output(captured, "Vulkan")
            .expect_err("the capture lacks the cursor move needed to start the map drag");
        assert!(full.contains("ui-drag-cursor-move-injected"), "{full}");

        let doctored = captured.replace(
            "ui-cursor-move-injected\nui-click-injected",
            "ui-click-injected\nui-cursor-move-injected",
        );
        let error = ui_pointer_streams::validate_combined_output(&doctored, "Vulkan")
            .expect_err("the exact live path must reject a reversed marker order");
        assert!(error.contains("ui-click-injected"), "{error}");
    }
}
