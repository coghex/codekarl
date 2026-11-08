//! The theme's frames and widgets (milestone 8, slice 7b) through a real UI VM over a recording scene: the quads of a cut-corner frame
//! as literals for each cut and corner set at two scales, the tints of a button's states, the window chrome of a place window, and
//! the text colour. The literals are the design's table (`docs/ui-design.md` §3.7, spec §4.3), computed independently of the library.

use crate::readout_tests::Readout;

/// The recorded lines of a script's `record` calls.
fn records(readout: &Readout) -> Vec<String> {
    readout
        .entries()
        .into_iter()
        .filter_map(|entry| entry.strip_prefix("record ").map(str::to_owned))
        .collect()
}

/// A frame 20 by 18 UI pixels at (10, 20) points, filled `#112233` and outlined `#445566`: every quad with its texture, rectangle and
/// the role its tint says (`fill` or `line`), in painter order.
fn frame_lines(scale: i64, cut: u32, corners: &str) -> Vec<String> {
    let readout = Readout::script(
        &format!(
            "local frame = require('lib.frame')\n\
             frame.new{{x = 10, y = 20, width = {}, height = {}, cut = {cut}, corners = '{corners}', fill = '#112233', line = '#445566'}}",
            20 * scale,
            18 * scale
        ),
        Some(scale),
    );
    let tints = readout.tints();
    readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("create "))
        .map(|entry| {
            let name = entry.split(' ').nth(1).expect("a quad name").to_owned();
            let colour = tints
                .iter()
                .find_map(|tint| tint.strip_prefix(&format!("create {name} ")))
                .expect("every frame quad is created with a tint");
            let role = match colour {
                "#112233" => "fill",
                "#445566" => "line",
                other => panic!("{other}"),
            };
            format!("{entry} {role}")
        })
        .collect()
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the design's table is written out for each cut and corner set at both scales"
)]
fn a_frames_quads_are_the_designs_table_for_each_cut_and_corner_set_at_two_scales() {
    let cases: &[(i64, u32, &str, &[&str])] = &[
        (
            2,
            3,
            "tl_br",
            &[
                "create q1 ui/theme/cut3_tl_fill.png 10 20 8 8 fill",
                "create q2 ui/theme/cut3_br_fill.png 42 48 8 8 fill",
                "create q3 ui/theme/white.png 18 22 30 6 fill",
                "create q4 ui/theme/white.png 12 28 36 20 fill",
                "create q5 ui/theme/white.png 12 48 30 6 fill",
                "create q6 ui/theme/cut3_tl_line.png 10 20 8 8 line",
                "create q7 ui/theme/cut3_br_line.png 42 48 8 8 line",
                "create q8 ui/theme/white.png 18 20 32 2 line",
                "create q9 ui/theme/white.png 48 22 2 26 line",
                "create q10 ui/theme/white.png 12 54 30 2 line",
                "create q11 ui/theme/white.png 10 28 2 28 line",
            ][..],
        ),
        (
            2,
            3,
            "tl",
            &[
                "create q1 ui/theme/cut3_tl_fill.png 10 20 8 8 fill",
                "create q2 ui/theme/white.png 18 22 30 6 fill",
                "create q3 ui/theme/white.png 12 28 36 26 fill",
                "create q4 ui/theme/cut3_tl_line.png 10 20 8 8 line",
                "create q5 ui/theme/white.png 18 20 32 2 line",
                "create q6 ui/theme/white.png 48 22 2 34 line",
                "create q7 ui/theme/white.png 12 54 36 2 line",
                "create q8 ui/theme/white.png 10 28 2 28 line",
            ][..],
        ),
        (
            2,
            6,
            "tl_br",
            &[
                "create q1 ui/theme/cut6_tl_fill.png 10 20 14 14 fill",
                "create q2 ui/theme/cut6_br_fill.png 36 42 14 14 fill",
                "create q3 ui/theme/white.png 24 22 24 12 fill",
                "create q4 ui/theme/white.png 12 34 36 8 fill",
                "create q5 ui/theme/white.png 12 42 24 12 fill",
                "create q6 ui/theme/cut6_tl_line.png 10 20 14 14 line",
                "create q7 ui/theme/cut6_br_line.png 36 42 14 14 line",
                "create q8 ui/theme/white.png 24 20 26 2 line",
                "create q9 ui/theme/white.png 48 22 2 20 line",
                "create q10 ui/theme/white.png 12 54 24 2 line",
                "create q11 ui/theme/white.png 10 34 2 22 line",
            ][..],
        ),
        (
            2,
            6,
            "tl",
            &[
                "create q1 ui/theme/cut6_tl_fill.png 10 20 14 14 fill",
                "create q2 ui/theme/white.png 24 22 24 12 fill",
                "create q3 ui/theme/white.png 12 34 36 20 fill",
                "create q4 ui/theme/cut6_tl_line.png 10 20 14 14 line",
                "create q5 ui/theme/white.png 24 20 26 2 line",
                "create q6 ui/theme/white.png 48 22 2 34 line",
                "create q7 ui/theme/white.png 12 54 36 2 line",
                "create q8 ui/theme/white.png 10 34 2 22 line",
            ][..],
        ),
        (
            2,
            7,
            "tl_br",
            &[
                "create q1 ui/theme/cut7_tl_fill.png 10 20 16 16 fill",
                "create q2 ui/theme/cut7_br_fill.png 34 40 16 16 fill",
                "create q3 ui/theme/white.png 26 22 22 14 fill",
                "create q4 ui/theme/white.png 12 36 36 4 fill",
                "create q5 ui/theme/white.png 12 40 22 14 fill",
                "create q6 ui/theme/cut7_tl_line.png 10 20 16 16 line",
                "create q7 ui/theme/cut7_br_line.png 34 40 16 16 line",
                "create q8 ui/theme/white.png 26 20 24 2 line",
                "create q9 ui/theme/white.png 48 22 2 18 line",
                "create q10 ui/theme/white.png 12 54 22 2 line",
                "create q11 ui/theme/white.png 10 36 2 20 line",
            ][..],
        ),
        (
            2,
            7,
            "tl",
            &[
                "create q1 ui/theme/cut7_tl_fill.png 10 20 16 16 fill",
                "create q2 ui/theme/white.png 26 22 22 14 fill",
                "create q3 ui/theme/white.png 12 36 36 18 fill",
                "create q4 ui/theme/cut7_tl_line.png 10 20 16 16 line",
                "create q5 ui/theme/white.png 26 20 24 2 line",
                "create q6 ui/theme/white.png 48 22 2 34 line",
                "create q7 ui/theme/white.png 12 54 36 2 line",
                "create q8 ui/theme/white.png 10 36 2 20 line",
            ][..],
        ),
        (
            3,
            3,
            "tl_br",
            &[
                "create q1 ui/theme/cut3_tl_fill.png 9 21 12 12 fill",
                "create q2 ui/theme/cut3_br_fill.png 57 63 12 12 fill",
                "create q3 ui/theme/white.png 21 24 45 9 fill",
                "create q4 ui/theme/white.png 12 33 54 30 fill",
                "create q5 ui/theme/white.png 12 63 45 9 fill",
                "create q6 ui/theme/cut3_tl_line.png 9 21 12 12 line",
                "create q7 ui/theme/cut3_br_line.png 57 63 12 12 line",
                "create q8 ui/theme/white.png 21 21 48 3 line",
                "create q9 ui/theme/white.png 66 24 3 39 line",
                "create q10 ui/theme/white.png 12 72 45 3 line",
                "create q11 ui/theme/white.png 9 33 3 42 line",
            ][..],
        ),
        (
            3,
            3,
            "tl",
            &[
                "create q1 ui/theme/cut3_tl_fill.png 9 21 12 12 fill",
                "create q2 ui/theme/white.png 21 24 45 9 fill",
                "create q3 ui/theme/white.png 12 33 54 39 fill",
                "create q4 ui/theme/cut3_tl_line.png 9 21 12 12 line",
                "create q5 ui/theme/white.png 21 21 48 3 line",
                "create q6 ui/theme/white.png 66 24 3 51 line",
                "create q7 ui/theme/white.png 12 72 54 3 line",
                "create q8 ui/theme/white.png 9 33 3 42 line",
            ][..],
        ),
        (
            3,
            6,
            "tl_br",
            &[
                "create q1 ui/theme/cut6_tl_fill.png 9 21 21 21 fill",
                "create q2 ui/theme/cut6_br_fill.png 48 54 21 21 fill",
                "create q3 ui/theme/white.png 30 24 36 18 fill",
                "create q4 ui/theme/white.png 12 42 54 12 fill",
                "create q5 ui/theme/white.png 12 54 36 18 fill",
                "create q6 ui/theme/cut6_tl_line.png 9 21 21 21 line",
                "create q7 ui/theme/cut6_br_line.png 48 54 21 21 line",
                "create q8 ui/theme/white.png 30 21 39 3 line",
                "create q9 ui/theme/white.png 66 24 3 30 line",
                "create q10 ui/theme/white.png 12 72 36 3 line",
                "create q11 ui/theme/white.png 9 42 3 33 line",
            ][..],
        ),
        (
            3,
            6,
            "tl",
            &[
                "create q1 ui/theme/cut6_tl_fill.png 9 21 21 21 fill",
                "create q2 ui/theme/white.png 30 24 36 18 fill",
                "create q3 ui/theme/white.png 12 42 54 30 fill",
                "create q4 ui/theme/cut6_tl_line.png 9 21 21 21 line",
                "create q5 ui/theme/white.png 30 21 39 3 line",
                "create q6 ui/theme/white.png 66 24 3 51 line",
                "create q7 ui/theme/white.png 12 72 54 3 line",
                "create q8 ui/theme/white.png 9 42 3 33 line",
            ][..],
        ),
        (
            3,
            7,
            "tl_br",
            &[
                "create q1 ui/theme/cut7_tl_fill.png 9 21 24 24 fill",
                "create q2 ui/theme/cut7_br_fill.png 45 51 24 24 fill",
                "create q3 ui/theme/white.png 33 24 33 21 fill",
                "create q4 ui/theme/white.png 12 45 54 6 fill",
                "create q5 ui/theme/white.png 12 51 33 21 fill",
                "create q6 ui/theme/cut7_tl_line.png 9 21 24 24 line",
                "create q7 ui/theme/cut7_br_line.png 45 51 24 24 line",
                "create q8 ui/theme/white.png 33 21 36 3 line",
                "create q9 ui/theme/white.png 66 24 3 27 line",
                "create q10 ui/theme/white.png 12 72 33 3 line",
                "create q11 ui/theme/white.png 9 45 3 30 line",
            ][..],
        ),
        (
            3,
            7,
            "tl",
            &[
                "create q1 ui/theme/cut7_tl_fill.png 9 21 24 24 fill",
                "create q2 ui/theme/white.png 33 24 33 21 fill",
                "create q3 ui/theme/white.png 12 45 54 27 fill",
                "create q4 ui/theme/cut7_tl_line.png 9 21 24 24 line",
                "create q5 ui/theme/white.png 33 21 36 3 line",
                "create q6 ui/theme/white.png 66 24 3 51 line",
                "create q7 ui/theme/white.png 12 72 54 3 line",
                "create q8 ui/theme/white.png 9 45 3 30 line",
            ][..],
        ),
    ];
    for (scale, cut, corners, expected) in cases {
        assert_eq!(
            frame_lines(*scale, *cut, corners),
            *expected,
            "P={scale} cut={cut} {corners}"
        );
    }
}

#[test]
fn a_frame_counts_eleven_quads_cut_at_both_corners_and_eight_at_the_top_left_only() {
    assert_eq!(frame_lines(2, 7, "tl_br").len(), 11);
    assert_eq!(frame_lines(2, 7, "tl").len(), 8);
    let fill = |lines: Vec<String>| lines.iter().filter(|line| line.ends_with(" fill")).count();
    assert_eq!(fill(frame_lines(2, 3, "tl_br")), 5);
    assert_eq!(fill(frame_lines(2, 3, "tl")), 3);
}

#[test]
fn a_frame_below_its_minimum_size_is_refused_and_a_bad_cut_or_corner_set_too() {
    let readout = Readout::script(
        r"
local frame = require('lib.frame')
local function try(spec)
  local ok, err = pcall(frame.new, spec)
  window.record(tostring(ok) .. ' ' .. tostring(err):gsub('^.-frame', 'frame', 1))
end
-- At two points per pixel: 2c + 3 pixels for both corners, c + 3 for the top-left only.
try{x = 0, y = 0, width = 2 * 16, height = 2 * 20, cut = 7, corners = 'tl_br', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 20, height = 2 * 16, cut = 7, corners = 'tl_br', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 17, height = 2 * 17, cut = 7, corners = 'tl_br', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 9, height = 2 * 9, cut = 7, corners = 'tl', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 10, height = 2 * 10, cut = 7, corners = 'tl', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 20, height = 2 * 20, cut = 5, corners = 'tl', fill = '#000000', line = '#ffffff'}
try{x = 0, y = 0, width = 2 * 20, height = 2 * 20, cut = 3, corners = 'br', fill = '#000000', line = '#ffffff'}
",
        None,
    );
    let lines = records(&readout);
    assert_eq!(lines.len(), 7);
    assert!(
        lines[0].starts_with("false ")
            && lines[0].contains("16x20 UI pixels is below the 17x17 minimum of a cut-7 frame"),
        "{lines:?}"
    );
    assert!(
        lines[1].contains("20x16 UI pixels is below the 17x17 minimum"),
        "{lines:?}"
    );
    assert!(lines[2].starts_with("true "), "{lines:?}");
    assert!(
        lines[3].contains("9x9 UI pixels is below the 10x10 minimum of a cut-7 frame"),
        "{lines:?}"
    );
    assert!(lines[4].starts_with("true "), "{lines:?}");
    assert!(lines[5].contains("cut must be 3, 6 or 7"), "{lines:?}");
    assert!(lines[6].contains("corners must be"), "{lines:?}");
}

#[test]
fn a_frames_set_colours_retints_only_the_quads_whose_colour_changes_and_set_rect_moves_only_what_moves()
 {
    let readout = Readout::script(
        r"
local frame = require('lib.frame')
f = frame.new{x = 10, y = 20, width = 40, height = 36, cut = 3, corners = 'tl_br', fill = '#112233', line = '#445566'}
window.record('mark')
f:set_colours('#112233', '#445566')
window.record('mark')
f:set_colours('#aabbcc', '#445566')
window.record('mark')
f:set_colours('#aabbcc', '#ddeeff')
window.record('mark')
f:set_rect(10, 20, 40, 36)
window.record('mark')
f:set_rect(10, 20, 60, 36)
window.record('mark')
",
        None,
    );
    // The same colours: no call.
    assert!(readout.tints_after_mark(1).is_empty());
    // The fill alone changes: the five fill quads (q1 to q5), in painter order.
    assert_eq!(
        readout.tints_after_mark(2),
        [
            "set_tint q1 #aabbcc",
            "set_tint q2 #aabbcc",
            "set_tint q3 #aabbcc",
            "set_tint q4 #aabbcc",
            "set_tint q5 #aabbcc"
        ]
    );
    // The line alone changes: the six line quads.
    assert_eq!(
        readout.tints_after_mark(3),
        [
            "set_tint q6 #ddeeff",
            "set_tint q7 #ddeeff",
            "set_tint q8 #ddeeff",
            "set_tint q9 #ddeeff",
            "set_tint q10 #ddeeff",
            "set_tint q11 #ddeeff"
        ]
    );
    // The same rectangle: no call. A wider frame moves the quads at its right and stretches the bands and edges, and leaves the
    // quads at the top-left (the tiles and the left edge) where they are.
    assert!(readout.after_mark(4).is_empty());
    let wider = readout.after_mark(5);
    assert!(!wider.is_empty() && wider.iter().all(|line| line.starts_with("set_rect ")));
    assert!(
        wider
            .iter()
            .all(|line| !line.starts_with("set_rect q1 ") && !line.starts_with("set_rect q11 ")),
        "{wider:?}"
    );
}

#[test]
fn a_texts_default_tint_is_the_themes_text_colour_and_set_colour_retints_every_glyph_and_the_next()
{
    let readout = Readout::script(
        r"
local text = require('lib.text')
local line = text.line(10, 20)
text.set(line, 'Go')
window.record('mark')
text.set_colour(line, '#15110a')
window.record('mark')
text.set_colour(line, '#15110a')
window.record('mark')
text.set(line, 'Gov')
window.record('mark')
local explicit = text.line(0, 0, nil, '#4fb3c8')
text.set(explicit, 'A')
",
        None,
    );
    let tints = readout.tints();
    assert_eq!(tints[..2], ["create q1 #ece3cb", "create q2 #ece3cb"]);
    assert_eq!(
        readout.tints_after_mark(1),
        ["set_tint q1 #15110a", "set_tint q2 #15110a"]
    );
    // The same colour again: no call.
    assert!(readout.tints_after_mark(2).is_empty());
    // A glyph created later takes the line's colour.
    assert_eq!(readout.tints_after_mark(3), ["create q3 #15110a"]);
    assert_eq!(readout.tints_after_mark(4), ["create q4 #4fb3c8"]);
}

#[test]
fn an_empty_label_and_a_narrow_one_give_a_nine_pixel_button_with_the_text_centred() {
    let readout = Readout::script(
        r"
local button = require('lib.button')
for _, content in ipairs{'', 'i', 'Go'} do
  local b = button.new{x = 10, y = 20, text = content, kind = 'place', style = 'title'}
  window.record(content .. ': ' .. table.concat({b:size()}, ' '))
end
",
        None,
    );
    let lines = records(&readout);
    // 9 by 15 UI pixels: 18 by 30 points; a 'Go' button is its text plus 2 pixels on each side.
    assert_eq!(lines[0], ": 18 30");
    assert!(lines[1].starts_with("i: 18 30"), "{lines:?}");
    assert_eq!(lines[2], "Go: 32 30");
    // 'i' is narrower than the frame's inside: its glyph starts at 2 + floor((9 - 4 - width) / 2) pixels. The glyph is the first
    // glyph quad after the second button's eleven frame quads: q23 (the first button has frame q1 to q11 and no glyph).
    let creates: Vec<String> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("create q23 "))
        .collect();
    assert_eq!(creates.len(), 1);
    let glyph_x: i64 = creates[0]
        .split(' ')
        .nth(3)
        .expect("an x")
        .parse()
        .expect("a number");
    let glyph_width: i64 = creates[0]
        .split(' ')
        .nth(5)
        .expect("a width")
        .parse()
        .expect("a number");
    // The glyph (a quad of `glyph_width` points) sits inside the 18-point button: centred within the 2-pixel padding.
    assert!(
        glyph_x >= 10 + 4 && glyph_x + glyph_width <= 10 + 18 - 4,
        "{creates:?}"
    );
}

#[test]
fn the_title_style_and_the_panel_style_have_the_tints_of_the_design_in_each_state() {
    // A button's states: (hovered, pressed) -> the fill, the line and the text colour of the design's table. The place kind has the
    // cyan outline, `hot` light cyan, and dark `pressed_text` and `title_text`.
    let readout = Readout::script(
        r"
local button = require('lib.button')
local pointer = require('lib.pointer')
a = button.new{x = 0, y = 0, text = 'A', kind = 'place'}
b = button.new{x = 100, y = 0, text = 'B', kind = 'place', style = 'title'}
local function look(name)
  window.record(name)
end
pointer.dispatch({over = a.key, overflow = false, held = false, events = {}})
look('a hover')
pointer.dispatch({over = a.key, overflow = false, held = true, events = {{kind = 'down', key = a.key, x = 1, y = 1, inside = true}}})
look('a pressed')
pointer.dispatch({over = false, overflow = false, held = false, events = {{kind = 'cancel', key = a.key, x = 1, y = 1}}})
look('a normal')
pointer.dispatch({over = b.key, overflow = false, held = false, events = {}})
look('b hover')
pointer.dispatch({over = b.key, overflow = false, held = true, events = {{kind = 'down', key = b.key, x = 101, y = 1, inside = true}}})
look('b pressed')
",
        None,
    );
    let tints = readout.tints();
    // Creation: panel look (fill body, line outline, text), then the title look (fill and line outline, dark title text).
    let created: Vec<&str> = tints[..24].iter().map(String::as_str).collect();
    assert_eq!(created[0], "create q1 #15110a");
    assert_eq!(created[5], "create q6 #4fb3c8");
    assert_eq!(
        created[11], "create q12 #ece3cb",
        "the panel button's glyph"
    );
    assert_eq!(
        created[12], "create q13 #4fb3c8",
        "the title button's fill is the outline colour"
    );
    assert_eq!(created[17], "create q18 #4fb3c8");
    assert_eq!(
        created[23], "create q24 #15110a",
        "the title text is dark on the cyan bar"
    );
    let after: Vec<&str> = tints[24..].iter().map(String::as_str).collect();
    // hover (panel): the line goes hot; pressed: the fill goes hot and the text takes `pressed_text`; cancel: back to normal.
    assert_eq!(after[0], "set_tint q6 #92ddea");
    assert!(after.contains(&"record a hover"));
    let at = |marker: &str| after.iter().position(|line| *line == marker).expect(marker);
    assert_eq!(after[at("record a hover") + 1], "set_tint q1 #92ddea");
    assert!(
        after[at("record a hover") + 1..at("record a pressed")].contains(&"set_tint q12 #15110a"),
        "{after:?}"
    );
    assert!(
        after[at("record a pressed") + 1..at("record a normal")].contains(&"set_tint q12 #ece3cb"),
        "{after:?}"
    );
    // The title button on hover takes the body fill and the hot line and the body-light text; pressed: hot fill and line, dark text.
    let hover = &after[at("record a normal") + 1..at("record b hover")];
    assert!(hover.contains(&"set_tint q13 #15110a"), "{hover:?}");
    assert!(hover.contains(&"set_tint q18 #92ddea"), "{hover:?}");
    assert!(hover.contains(&"set_tint q24 #ece3cb"), "{hover:?}");
    let pressed = &after[at("record b hover") + 1..at("record b pressed")];
    assert!(pressed.contains(&"set_tint q13 #92ddea"), "{pressed:?}");
    assert!(pressed.contains(&"set_tint q24 #15110a"), "{pressed:?}");
}

#[test]
fn nothing_loads_the_old_button_art_and_the_time_bar_and_tooltip_load_the_themes() {
    let mut readout = Readout::new("", None);
    readout.deliver(&crate::readout_tests::snapshot(
        14,
        3,
        Some(crate::game::slot::PauseReason::Player),
    ));
    readout.deliver_hover(Some(("town", "Alpha", 700, 760, 80)));
    let loads: Vec<String> = readout
        .entries()
        .into_iter()
        .filter(|entry| entry.starts_with("load "))
        .collect();
    assert!(
        loads.iter().all(|load| !load.contains("ui/button/")),
        "{loads:?}"
    );
    assert!(
        loads.contains(&"load ui/theme/white.png".to_owned()),
        "{loads:?}"
    );
    assert!(
        loads.contains(&"load ui/theme/cut3_tl_fill.png".to_owned()),
        "{loads:?}"
    );
}
