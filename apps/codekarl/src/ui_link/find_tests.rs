//! Find's search model runs in the real UI VM.
use super::listbar_tests::rig;

#[test]
fn search_folds_accents_sorts_ties_and_caps_matches() {
    let mut rig = rig(
        "",
        &[r#"
local find = require("lib.find_model")
local entries = {
  {kind="river", id="r", name="Máel"},
  {kind="character", id=2, name="MAEL", place="Grianán Ailigh"},
  {kind="location", id="l", name="mael"},
  {kind="location", id="x", name="Other"},
}
local rows, more = find.search(entries, "  mael  ")
assert(#rows == 3 and more == 0)
assert(rows[1].kind == "location" and rows[2].kind == "character" and rows[3].kind == "river")
assert(rows[2].place == "Grianán Ailigh")
assert(find.index_of(rows, rows[2]) == 2)
assert(find.index_of({rows[3], rows[2]}, rows[2]) == 2)
assert(find.index_of({rows[1], rows[3]}, rows[2]) == nil)
assert(#find.search(entries, "") == 0)
assert(#find.search(entries, "nothing") == 0)
local many = {}
for i = 1, 205 do many[i] = {kind="character", id=i, name="Same"} end
rows, more = find.search(many, "same")
assert(#rows == 200 and more == 5 and rows[1].id == 1 and rows[200].id == 200)
"#],
    );
    rig.opened();
}

#[test]
fn popup_keeps_the_query_and_reconciles_the_highlight_by_identity() {
    let mut rig = rig(
        "",
        &[r#"
local find = require("lib.find_popup")
local data = {
  {kind="location", id="a", name="Áilech"},
  {kind="location", id="b", name="Ailne"},
}
local block = false
game = {set_find_keyboard_block=function(on) block=on end,
        find_entries=function(_) return data end,
        go_to=function() end}
local view = {width=800, height=600}
find.open(view, 1, 0)
local popup = find.current()
assert(block and popup.field:editing())
popup.field:on_typed({kind="char", text="a"})
assert(#popup.rows == 2 and popup.highlight.id == "a")
popup.field:on_typed({kind="char", text="i"})
popup.field:on_typed({kind="left"})
popup.field:on_typed({kind="delete"})
assert(table.concat(popup.field.chars) == "a" and #popup.rows == 2)
data = {{kind="location", id="b", name="Ailne"}}
find.set_revision(2, 0)
assert(popup.highlight == nil and #popup.rows == 1 and popup.field:editing())
find.set_revision(3, 0)
popup.field:on_typed({kind="enter"})
assert(popup.message == "no longer there" and popup.field:editing())
find.close()
assert(not block and find.current() == nil)
find.open(view, 2, 0)
assert(#find.current().rows == 0)
find.close()
"#],
    );
    rig.opened();
}

#[test]
fn typed_stream_filters_then_moves_and_selects_in_order() {
    use super::numberfield_tests::{input_applied, update};
    use ck_window::TypedEntry;
    let body = r#"
local find = require("lib.find_popup")
local typing = require("lib.typing")
local actual = window.set_text_input
window.set_text_input = function(on) local gen = actual(on); note("input " .. tostring(on) .. " " .. gen); return gen end
chosen = nil
game = {set_find_keyboard_block=function() end,
 find_entries=function() return {{kind="river", id="a", name="Ailech"}, {kind="river", id="b", name="Ailne"}} end,
 go_to=function(kind, id, zoom, revision) chosen={kind,id,zoom,revision} end}
frame.on_update(function(update) typing.dispatch(update); pointer_lib.dispatch(update.pointer) end)
"#;
    let mut rig = rig(
        body,
        &[
            "find_popup = require('lib.find_popup'); find_popup.open({width=800,height=600}, 7, 0)",
            "assert(chosen and chosen[1]=='river' and chosen[2]=='b' and chosen[3] and chosen[4]==7); assert(find_popup.current()==nil)",
        ],
    );
    rig.opened();
    input_applied(&mut rig);
    update(
        &mut rig,
        &[TypedEntry::Char('a'), TypedEntry::Down, TypedEntry::Enter],
        &[],
    );
    rig.opened();
}

#[test]
fn queued_tagged_click_cannot_activate_the_replacement_row() {
    let mut rig = rig(
        "",
        &[r#"
local find = require("lib.find_popup")
local pointer = require("lib.pointer")
local goto_lib = require("lib.go_to")
local calls = 0
local data = {{kind="location", id="old", name="Ailech"}}
game = {set_find_keyboard_block=function() end, find_entries=function() return data end,
  go_to=function() calls=calls+1 end}
find.open({width=800,height=600}, 1, 0)
local p = find.current()
p.field:on_typed({kind="char", text="a"})
local key, tag = p.list.row_keys[1], "f:" .. goto_lib.tag("location", "old", 1, 0):sub(4)
data = {{kind="location", id="new", name="Ailne"}}
find.set_revision(2, 0)
pointer.dispatch({events={{kind="down", key=key, tag=tag}}, over=false})
assert(calls == 0 and p.message == "no longer there" and p.field:editing())
find.close()
"#],
    );
    rig.opened();
}

#[test]
fn inside_clicks_keep_editing_and_the_outside_click_closes() {
    use super::listbar_tests::{click_at, point};
    let body = r#"
local find = require("lib.find_popup")
local typing = require("lib.typing")
game = {set_find_keyboard_block=function() end, find_entries=function() return {} end}
frame.on_update(function(update) typing.dispatch(update); pointer_lib.dispatch(update.pointer) end)
"#;
    let mut rig = rig(
        body,
        &[
            r"find = require('lib.find_popup'); find.open({width=800,height=600}, 1, 0)
local p=find.current(); local x,y=p.window:content_origin()
mark('list', x+10*P, y+30*P); mark('frame', p.window.x+2*P, p.window.y+20*P)
mark('outside', 20, 20)",
            "assert(find.current() and find.current().field:editing())",
            "assert(find.current() and find.current().field:editing())",
            "assert(find.current() == nil)",
        ],
    );
    rig.opened();
    let list = point(&rig, "list");
    click_at(&mut rig, list);
    rig.opened();
    let frame = point(&rig, "frame");
    click_at(&mut rig, frame);
    rig.opened();
    let outside = point(&rig, "outside");
    click_at(&mut rig, outside);
    rig.opened();
}

#[test]
fn a_real_left_click_selects_the_tagged_result() {
    use super::listbar_tests::{click_at, point};
    let body = r#"
local typing = require("lib.typing")
game = {set_find_keyboard_block=function() end,
  find_entries=function() return {{kind="river", id="erne", name="Erne"}} end,
  go_to=function(kind,id,zoom,revision) chosen={kind,id,zoom,revision} end}
frame.on_update(function(update) typing.dispatch(update); pointer_lib.dispatch(update.pointer) end)
"#;
    let mut rig = rig(
        body,
        &[
            r"find=require('lib.find_popup'); find.open({width=800,height=600}, 3, 0)
local p=find.current(); p.field:on_typed({kind='char',text='e'})
local x,y=p.window:content_origin(); mark('row',x+20*P,y+24*P)",
            "assert(chosen and chosen[1]=='river' and chosen[2]=='erne' and chosen[3] and chosen[4]==3); assert(find.current()==nil)",
        ],
    );
    rig.opened();
    let row = point(&rig, "row");
    click_at(&mut rig, row);
    rig.opened();
}

#[test]
fn a_row_press_dragged_away_does_not_select() {
    use super::listbar_tests::{frame, point};
    use super::widget_witness_tests::points;
    use ck_window::PointerEvent;
    let body = r#"
local typing = require("lib.typing")
game = {set_find_keyboard_block=function() end,
  find_entries=function() return {{kind="river", id="erne", name="Erne"}} end,
  go_to=function() chosen=true end}
frame.on_update(function(update) typing.dispatch(update); pointer_lib.dispatch(update.pointer) end)
"#;
    let mut rig = rig(
        body,
        &[
            r"find=require('lib.find_popup'); find.open({width=800,height=600}, 3, 0)
local p=find.current(); p.field:on_typed({kind='char',text='e'})
local x,y=p.window:content_origin(); mark('row',x+20*P,y+24*P)",
            "assert(chosen == nil and find.current() ~= nil)",
            "assert(chosen == nil and find.current() ~= nil); find.close()",
        ],
    );
    rig.opened();
    let (x, y) = point(&rig, "row");
    rig.feed(&[PointerEvent::Down(points(x, y))]);
    frame(&mut rig);
    rig.opened();
    rig.feed(&[PointerEvent::Up(points(20.0, 20.0))]);
    frame(&mut rig);
    rig.opened();
}

#[test]
fn a_refused_goto_keeps_the_query_and_a_new_world_closes_the_popup() {
    let mut rig = rig(
        "",
        &[r"
local find = require('lib.find_popup')
local blocks = {}
game = {set_find_keyboard_block=function(on) blocks[#blocks+1]=on end,
 find_entries=function() return {{kind='river',id='erne',name='Erne'}} end,
 go_to=function() error('retired') end}
find.open({width=800,height=600}, 1, 0)
local p=find.current()
p.field:on_typed({kind='char',text='e'})
p.field:on_typed({kind='enter'})
assert(find.current()==p and p.field:editing() and table.concat(p.field.chars)=='e' and p.message=='no longer there')
find.set_revision(2, 1)
assert(find.current()==nil and blocks[1] and not blocks[2])
"],
    );
    rig.opened();
}

#[test]
fn find_action_waits_until_the_update_after_editing_has_ended() {
    let mut rig = rig(
        "",
        &[r"
local find=require('lib.find_popup')
local typing=require('lib.typing')
game={find_entries=function() return {} end, set_find_keyboard_block=function() end}
local view={width=800,height=600}
local press={actions={game_find={pressed=true}}}
find.set_revision(1,0)
find.update({actions={}},view)
local editor={on_typed=function() end}
typing.start(editor)
find.update(press,view)
assert(find.current()==nil)
typing.stop(editor)
find.update(press,view)
assert(find.current()==nil)
find.update(press,view)
assert(find.current() and find.current().field:editing())
find.close()
"],
    );
    rig.opened();
}

#[test]
fn highlight_follows_identity_and_selection_uses_the_new_revision() {
    let mut rig = rig(
        "",
        &[r"
local find=require('lib.find_popup')
local data={{kind='river',id='a',name='A'}, {kind='river',id='b',name='B'}}
local got
game={find_entries=function() return data end, set_find_keyboard_block=function() end,
  go_to=function(kind,id,zoom,revision) got={kind,id,revision} end}
find.open({width=800,height=600},1,0)
local p=find.current(); p.field:on_typed({kind='char',text='a'})
assert(p.highlight.id=='a')
data={{kind='river',id='z',name='AA'}, {kind='river',id='a',name='ZA'}}
find.set_revision(2,0)
assert(p.highlight.id=='a' and p.list:selected()==2)
p.field:on_typed({kind='enter'})
assert(got[1]=='river' and got[2]=='a' and got[3]==2 and find.current()==nil)
"],
    );
    rig.opened();
}
