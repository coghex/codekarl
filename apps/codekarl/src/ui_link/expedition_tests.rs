//! The expedition map layer in the real script rig, without a panel.
use super::listbar_tests::{click, frame, rig};
use super::widget_witness_tests::points;
use ck_window::{PointerEvent, ScrollUnit};

#[test]
fn chips_paths_selection_and_layer_hiding_follow_the_map_readback() {
    let mut rig = rig(
        r#"
layer = require("lib.expeditions").new()
data = {world = "0", shown = true, trips = {
  {id = "1", icon = "hand_cart", position = {100, 100}, past = {{{80, 100}, {100, 100}}}, ahead = {{{100, 100}, {120, 100}}}},
  {id = "2", icon = "ox_cart", position = {100, 100}, past = {{{80, 100}, {100, 100}}}, ahead = {{{100, 100}, {120, 100}}}},
  {id = "3", icon = "covered_wagon", position = {200, 100}, past = {}, ahead = {}},
  {id = "4", icon = "simple_barge", position = {300, 100}, past = {}, ahead = {}},
  {id = "5", icon = "currach", position = {400, 100}, past = {}, ahead = {}},
  {id = "6", icon = "on_foot", position = {500, 100}, past = {}, ahead = {}},
}}
layer:update(data, 800, 600)
"#,
        &[
            r#"
local first, second, textures = nil, nil, {}
for _, q in pairs(scene.quads) do
  if q.group == layer.chips then textures[q.texture] = q; eq(q.w, 42); eq(q.h, 44); eq(q.y, 80) end
  if q.texture == "ui/expeditions/hand_cart.png" then first = q end
  if q.texture == "ui/expeditions/ox_cart.png" then second = q end
end
expect(first and second)
for _, name in ipairs({"hand_cart", "ox_cart", "covered_wagon", "simple_barge", "currach", "on_foot"}) do
  expect(textures["ui/expeditions/" .. name .. ".png"] ~= nil, name)
end
eq(first.x, 80) eq(first.y, 80) eq(first.w, 42) eq(first.h, 44)
expect(first.seq < second.seq)
local count = 0
for _, hit in pairs(scene.hits) do
  if hit.group == layer.chips then
    local r = hit.rects[1]; eq(r.y, 80); eq(r.width, 42); eq(r.height, 44); count = count + 1
  end
end
eq(count, 6)
mark("chip", 100, 100)
"#,
            r##"
eq(layer.selected, "2")
layer:update(data, 800, 600)
local grey, yellow = false, false
for _, q in pairs(scene.quads) do
  if q.group == layer.paths and q.tint == "#a0a0a0" then grey = true; expect(q.x <= 100) end
  if q.group == layer.paths and q.tint == "#f0cd3c" then yellow = true; expect(q.x >= 100) end
end
expect(grey and yellow)
data.shown = false; data.trips = {}
layer:update(data, 800, 600)
eq(layer.selected, nil)
for _, hit in pairs(scene.hits) do expect(hit.group ~= layer.chips) end
"##,
        ],
    );
    rig.opened();
    click(&mut rig, "chip");
    rig.opened();
}

#[test]
fn selected_paths_survive_a_map_press_and_clear_only_on_map_click() {
    let mut rig = rig(
        r#"
layer = require("lib.expeditions").new()
data = {world = "0", shown = true, trips = {
  {id = "1", icon = "hand_cart", position = {100, 100}, past = {{{80, 100}, {100, 100}}}, ahead = {{{100, 100}, {120, 100}}}},
}}
layer:update(data, 800, 600)
"#,
        &[
            r#"mark("chip", 100, 100)"#,
            r#"
eq(layer.selected, "1")
layer:update(data, 800, 600)
local n = 0; for _, q in pairs(scene.quads) do if q.group == layer.paths then n = n + 1 end end
expect(n > 0)
"#,
            r#"
eq(layer.selected, "1")
local n = 0; for _, q in pairs(scene.quads) do if q.group == layer.paths then n = n + 1 end end
expect(n > 0)
"#,
            r"
eq(layer.selected, nil)
layer:update(data, 800, 600)
for _, q in pairs(scene.quads) do expect(q.group ~= layer.paths) end
",
        ],
    );
    rig.opened();
    click(&mut rig, "chip");
    rig.opened();
    rig.feed(&[
        PointerEvent::Down(points(20.0, 250.0)),
        PointerEvent::Move(points(40.0, 250.0)),
        PointerEvent::Up(points(40.0, 250.0)),
        PointerEvent::Scroll {
            position: points(20.0, 250.0),
            amount: 256,
            unit: ScrollUnit::Lines,
        },
    ]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[
        PointerEvent::Down(points(20.0, 250.0)),
        PointerEvent::Up(points(20.0, 250.0)),
    ]);
    frame(&mut rig);
    rig.opened();
}
