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

#[test]
fn expedition_panel_switches_tabs_and_draws_zero_capacity_without_a_fill() {
    let mut rig = rig(
        r"
local Panel = require('lib.expedition_panel')
panel = Panel.new{view={width=800,height=600},top=function() return 0 end,on_close=function() end,
  on_go=function() end,on_follow=function() end,open_vehicle=function() end}
data = {home='home', destination='away', trader='peasant', phase='outbound', pace_pm=1000, limiter='walkers', water=false,
 people={{pop=0,kind='peasant',count=1,own=false,roles={{role='walker',amount=1}},paid={{id='oats',amount=2},{id='fish',amount=1}}}},
 drovers=0,walkers=1,vehicles={},cargo={{id='sheep',amount=2,weight_g=70000,animal=true}}, supplies={},wanted={},
 load_g=0,load_ml=0,capacity_g=0,capacity_ml=0,travelled=0,total=0,arrives_text='—',trades_text='—',food_days=0,now=0,left=0}
panel:open('1','1',data)
",
        &[
            r"
local found = false
for _, line in ipairs(lines) do if not line.test_dead and line_text(line) == '—' then found = true end end
expect(found, 'zero capacity uses a dash')
local driven = false
for _, line in ipairs(lines) do if not line.test_dead and line_text(line):find('driven', 1, true) then driven = true end end
expect(driven, 'land animals are driven')
data.water = true; panel:refresh(data)
local aboard = false
for _, line in ipairs(lines) do if not line.test_dead and line_text(line):find('aboard', 1, true) then aboard = true end end
expect(aboard, 'water animals are aboard')
local tracks = 0
for _, q in pairs(scene.quads) do
  if q.tint == theme_lib.colour('track') and q.h == 6 then
    tracks = tracks + 1
    for _, fill in pairs(scene.quads) do
      assert(not (fill.group == q.group and fill.y == q.y and fill.h == q.h and fill.tint == theme_lib.colour('number')),
        'zero capacity has no fill')
    end
  end
end
expect(tracks >= 2, 'weight and space tracks are drawn')
data.water=false; data.capacity_g=25000; data.capacity_ml=50000; data.load_g=10000; data.load_ml=20000
panel:refresh(data)
local filled = 0
for _, q in pairs(scene.quads) do
  if q.tint == theme_lib.colour('track') and q.h == 6 then
    for _, fill in pairs(scene.quads) do
      if fill.group == q.group and fill.y == q.y and fill.h == q.h and fill.tint == theme_lib.colour('number') and fill.w == math.floor(q.w / 2 * .4) * 2 then filled = filled + 1 end
    end
  end
end
expect(filled >= 2, 'both pack bars show their load')
local x,y,w,h = hit_rect(panel.tabs.buttons[2].key); mark('crew',x+w/2,y+h/2)
",
            r"
eq(panel.tab, 'crew')
local oats, fish = false, false
for _, line in ipairs(lines) do if not line.test_dead then
  oats = oats or line_text(line):find('2 oats', 1, true) ~= nil
  fish = fish or line_text(line):find('1 fish', 1, true) ~= nil
end end
expect(oats and fish, 'both wage goods appear')
local x,y,w,h = hit_rect(panel.tabs.buttons[3].key); mark('vehicles',x+w/2,y+h/2)
",
            r"
eq(panel.tab, 'vehicles')
local foot = false
for _, line in ipairs(lines) do if not line.test_dead and line_text(line) == 'on foot' then foot = true end end
expect(foot, 'on foot card')
",
        ],
    );
    rig.opened();
    click(&mut rig, "crew");
    rig.opened();
    click(&mut rig, "vehicles");
    rig.opened();
}

#[test]
fn vehicle_detail_returns_to_trip_and_stays_if_trip_ends() {
    let mut rig = rig(
        r"
local Panel = require('lib.expedition_panel')
closed = 0
panel = Panel.new{view={width=800,height=600},top=function() return 0 end,on_close=function() closed=closed+1 end,
  on_go=function() end,on_follow=function() end}
data = {home='home', destination='away', trader='peasant', phase='outbound', pace_pm=800, limiter='ox_cart', water=false,
 people={{pop=0,kind='peasant',count=1,own=true,roles={{role='crew of ox_cart',amount=1}},paid={}}},
 drovers=0,walkers=0,vehicles={{number='7',kind='ox_cart',owner='peasant',materials={{id='hardwood',amount=4}},
 crew=1,full_crew=2,crew_fewest=1,build_hours=8,build_hands=2,land_pace_pm=800,water_profile=false,wear='1 hardwood per 30 days',load_g=0,load_ml=0,
 capacity_g=500000,capacity_ml=1500000,carrier=false}},cargo={},supplies={},wanted={},load_g=0,load_ml=0,
 capacity_g=500000,capacity_ml=1500000,travelled=1,total=5,arrives_text='date',trades_text='date',food_days=2,now=1,left=0}
panel:open('1','1',data)
",
        &[r"
panel.tab='vehicles'; panel:refresh(data); panel.stack.items[1].on_click(); pointer_lib.dispatch({})
expect(panel:is_vehicle()); eq(closed, 0)
panel:close(); expect(panel:is_open()); eq(panel.mode, 'expedition'); eq(panel.tab, 'vehicles'); eq(closed, 0)
panel:open_vehicle('7'); panel:trip_ended(); expect(panel:is_vehicle()); expect(panel.ended)
panel:close(); expect(not panel:is_open()); eq(closed, 1)
local returned = 0
panel:open_from_pop(data.vehicles[1], 'home', '1', function() returned = returned + 1 end)
expect(panel:is_vehicle()); panel:close(); eq(returned, 1); expect(not panel:is_open())
"],
    );
    rig.opened();
}

#[test]
fn queued_old_world_chip_click_does_not_select_the_new_world() {
    let mut rig = rig(
        r"
layer = require('lib.expeditions').new()
data = {world='old',shown=true,trips={{id='1',icon='on_foot',position={100,100},past={},ahead={}}}}
layer:update(data,800,600)
old_key = layer.hits['1']
data.world = 'new'; layer:update(data,800,600)
",
        &[r"
assert(layer.hits['1'] ~= old_key)
pointer_lib.dispatch({events={{kind='down',key=old_key,inside=true},{kind='up',key=old_key,inside=true}}})
assert(layer.selected == nil, 'a queued old-world click selects nothing')
"],
    );
    rig.opened();
}

#[test]
fn ended_vehicle_refreshes_once_per_snapshot_hour() {
    use super::group_tests::Rig;
    let main = include_str!("../../../../scripts/ui/main.lua")
        .replace("local expedition_layer = nil", "expedition_layer = nil")
        .replace("local expedition_panel = nil", "expedition_panel = nil")
        .replace("local expedition_update = nil", "expedition_update = nil");
    let prefix = r"
calls=0
game={on_time=function() end,on_select=function() end,on_hover=function() end,control=function() end,
 expeditions=function() return nil end,expedition_camera=function() return 0 end,
 pop_trade=function() calls=calls+1; return {vehicles={free_vehicle}} end}
";
    let suffix = r"
local vehicle={number='7',kind='hand_cart',owner='peasant',materials=false,free=false,expedition='1',carrier=false,
 crew=1,full_crew=1,crew_fewest=1,build_hours=4,build_hands=1,land_pace_pm=1000,water_profile=false,wear='1 hardwood',
 load_g=0,load_ml=0,capacity_g=150000,capacity_ml=500000}
free_vehicle={}
for key,value in pairs(vehicle) do free_vehicle[key]=value end
free_vehicle.free=true; free_vehicle.expedition=false
local trip={id='1',icon='hand_cart',position={100,100},past={},ahead={},panel={home='home',destination='away',trader='peasant',trader_slot=0,
 phase='outbound',pace_pm=1000,limiter='walkers',water=false,people={},drovers=0,walkers=0,
 vehicles={vehicle},cargo={},supplies={},wanted={},load_g=0,load_ml=0,capacity_g=0,capacity_ml=0,
 travelled=0,total=1,arrives_text='date',trades_text='date',food_days=0,now=0,left=0}}
local data={world='1',tick=1,shown=true,trips={trip}}
expedition_layer.selected='1'; expedition_update(data); expedition_panel:open_vehicle('7')
local refreshes=0
local refresh=expedition_panel.refresh
function expedition_panel:refresh(view) refreshes=refreshes+1; return refresh(self,view) end
data.trips={}; expedition_layer:clear(); expedition_update(data); expedition_update(data)
assert(expedition_panel.ended and calls==1 and refreshes==1, 'ending in the same hour refreshes once')
data.tick=2; expedition_update(data); expedition_update(data)
assert(calls==2 and refreshes==2, 'the next hour refreshes once')
";
    let mut rig = Rig::new(&format!("{prefix}\n{main}\n{suffix}"));
    rig.opened();
}

#[test]
fn production_follow_moves_hourly_and_stops_on_manual_input_and_layer_hide() {
    use super::group_tests::Rig;
    let main = include_str!("../../../../scripts/ui/main.lua")
        .replace("local expedition_layer = nil", "expedition_layer = nil")
        .replace("local expedition_panel = nil", "expedition_panel = nil")
        .replace("local expedition_update = nil", "expedition_update = nil")
        .replace("  local location_panel\n", "  location_panel = nil\n");
    let prefix = r"
manual = 0; moves = 0
callbacks = {}
game = {on_time=function(f) callbacks.time=f end, on_select=function() end, on_hover=function() end,
 control=function() end, expeditions=function() return nil end,
 expedition_camera=function(id) if id == false then return manual end; moves=moves+1; return manual end}
";
    let suffix = r"
data = {world='1',shown=true,tick=1,trips={{id='1',icon='on_foot',position={100,100},past={},ahead={},panel={
 home='home',destination='away',trader='peasant',phase='outbound',pace_pm=1000,limiter='walkers',water=false,
 people={{pop=0,kind='peasant',count=1,own=true,roles={{role='walker',amount=1}},paid={}}},drovers=0,walkers=1,
 vehicles={},cargo={},supplies={},wanted={},load_g=0,load_ml=0,capacity_g=25000,capacity_ml=50000,
 travelled=1,total=5,arrives_text='date',trades_text='date',food_days=1,now=1,left=0}}}}
local old = require('lib.window').new{kind='place',content_width=200,content_height=200}
location_panel.window = old
expedition_layer.selected='1'; expedition_update(data)
assert(old.closed, 'opening a trip replaces the prior location in the left slot')
assert(expedition_panel:is_open())
expedition_panel.window.title_buttons[1].on_click(); assert(moves == 1)
expedition_panel.follow_button.on_click(); assert(moves == 2 and expedition_panel.follow_button.selected)
data.tick=2; expedition_update(data); assert(moves == 3)
manual=manual+1; data.tick=3; expedition_update(data)
assert(moves == 3 and not expedition_panel.follow_button.selected, 'a hand pan stops follow before the next move')
expedition_panel.follow_button.on_click(); assert(moves == 4)
local trip = data.trips[1]
expedition_layer:clear(); data.shown=false; data.trips={}; expedition_update(data)
assert(not expedition_panel:is_open(), 'hiding the layer closes the panel and follow')
data.shown=true; data.trips={trip}; data.tick=4; expedition_layer.selected='1'; expedition_update(data)
expedition_panel.follow_button.on_click(); data.trips={}; expedition_layer:clear(); expedition_update(data)
assert(not expedition_panel:is_open(), 'unloading closes the panel and follow')
";
    let mut rig = Rig::new(&format!("{prefix}\n{main}\n{suffix}"));
    rig.opened();
}

#[test]
fn escape_closes_the_top_window_before_the_expedition_panel() {
    use super::group_tests::Rig;
    let main = include_str!("../../../../scripts/ui/main.lua")
        .replace("local expedition_layer = nil", "expedition_layer = nil")
        .replace("local expedition_panel = nil", "expedition_panel = nil")
        .replace("local expedition_update = nil", "expedition_update = nil");
    let prefix = r"
game = {on_time=function() end,on_select=function() end,on_hover=function() end,control=function() end,
 expeditions=function() return nil end,expedition_camera=function() return 0 end}
";
    let suffix = r"
local overlay
local stage = 0
window.on_opened(function()
  stage = stage + 1
  if stage == 1 then
    local trip={id='1',icon='on_foot',position={100,100},past={},ahead={},panel={home='home',destination='away',trader='peasant',
      phase='outbound',pace_pm=1000,limiter='walkers',water=false,people={},drovers=0,walkers=0,
      vehicles={{number='7',kind='hand_cart',owner='peasant',materials=false,free=false,expedition='1',carrier=false,
        crew=1,full_crew=1,crew_fewest=1,build_hours=4,build_hands=1,land_pace_pm=1000,water_profile=false,wear='1 hardwood',
        load_g=0,load_ml=0,capacity_g=150000,capacity_ml=500000}},cargo={},supplies={},wanted={},
      load_g=0,load_ml=0,capacity_g=0,capacity_ml=0,travelled=0,total=1,arrives_text='date',trades_text='date',food_days=0,now=0,left=0}}
    expedition_layer.selected='1'; expedition_update({world='1',tick=1,shown=true,trips={trip}})
    overlay=require('lib.window').new{title='other',content_width=200,content_height=200}
  elseif stage == 2 then
    assert(overlay.closed and expedition_panel:is_open(), 'Escape closes the top window first')
    expedition_panel:open_vehicle('7')
  elseif stage == 3 then
    assert(expedition_panel:is_open() and expedition_panel.mode == 'expedition' and expedition_panel.tab == 'vehicles', 'Escape returns to the expedition')
    expedition_panel:open_vehicle('7'); expedition_panel:trip_ended()
  else
    assert(not expedition_panel:is_open() and expedition_layer.selected == nil, 'Escape after unloading closes to the map')
  end
end)
";
    let mut rig = Rig::new_fixture_body(&format!("{prefix}\n{main}\n{suffix}"));
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    assert!(rig.escape());
    rig.opened();
    assert!(rig.escape());
    rig.opened();
}

#[test]
fn real_camera_refuses_a_stale_manual_generation_or_world() {
    use ck_lua::ConfigValue;
    use std::sync::Arc;
    let rig = super::group_tests::Rig::new_fixture_body("window.on_opened(function() end)");
    let center = crate::map_app::lock(&rig.map).camera().unwrap().center;
    let mut snapshot = crate::readout_tests::snapshot(0, 1, None);
    snapshot.world = 1;
    snapshot.layers.expeditions = true;
    snapshot.expeditions = Arc::new(vec![crate::game::expedition_map::ExpeditionMap {
        number: 7,
        icon: "on_foot".to_owned(),
        position: [i64::from(center.x), i64::from(center.y)],
        past: vec![],
        ahead: vec![],
        panel: None,
    }]);
    rig.simulation_slot.publish(snapshot.clone());
    let call = crate::expedition_api::camera_function(
        Arc::clone(&rig.simulation_slot),
        Some(Arc::clone(&rig.map)),
    );
    let args = |world: &str, generation: i64| {
        vec![
            Some(ConfigValue::String("7".to_owned())),
            Some(ConfigValue::String(world.to_owned())),
            Some(ConfigValue::Bool(false)),
            Some(ConfigValue::Integer(generation)),
        ]
    };
    assert!(
        call(&args("0", 0)).is_err(),
        "old worlds cannot move the camera"
    );
    assert!(
        call(&args("1", 1)).is_err(),
        "a stale follow generation cannot move the camera"
    );
    assert!(call(&args("1", 0)).is_ok());
    snapshot.layers.expeditions = false;
    rig.simulation_slot.publish(snapshot);
    assert!(
        call(&args("1", 0)).is_err(),
        "a hidden layer cannot move the camera"
    );
}
