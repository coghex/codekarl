//! The map modes' window witness (milestone 9, slices 8 and 10b): the shipped `scripts/ui/main.lua` run unchanged over Ireland 768 with a
//! few wrappers installed before it. Once the UI has had a frame, the wrapper presses the three map-mode buttons' own handlers in turn
//! (stockpile, yield, yield again to turn the mode off, stockpile, elevation in its place, elevation again to turn it off), a fixed
//! number of frames apart, logging each call of
//! `game.set_map_mode` and what the host answered, and then closes the window. The real presenter draws every mode off the real economy
//! views; a refused table or a refused map image would be logged, and the run must close normally.

use std::path::Path;

/// How many frames the wrapper waits between presses: the presenter has drawn several frames of each mode by the next.
const FRAMES_BETWEEN: u32 = 20;

/// The prefix: wrappers over `game.set_map_mode`, the button library and `frame.on_update`, installed before `main.lua` runs.
fn prefix() -> String {
    format!(
        r"
do -- scoped: the wrapper's own helpers stay what they are
local real_set_map_mode = game.set_map_mode
game.set_map_mode = function(name)
  local ok, err = pcall(real_set_map_mode, name)
  log.info('set_map_mode ' .. name .. ' ' .. (ok and 'ok' or ('refused ' .. tostring(err))))
  if not ok then error(err, 0) end
end

local button_library = require('lib.button')
local presses = {{}}
local real_button_new = button_library.new
button_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.icon == 'stockpile' or spec.icon == 'yield' or spec.icon == 'elevation' then presses[spec.icon] = spec.on_click end
  return created
end

-- Stockpile, yield, yield again (the mode that shows turns off), stockpile, elevation (it replaces the stockpile), elevation again (off).
local sequence = {{'stockpile', 'yield', 'yield', 'stockpile', 'elevation', 'elevation'}}
local frames, step = 0, 0
local real_on_update = frame.on_update
frame.on_update = function(callback)
  if callback == nil then return real_on_update(nil) end
  real_on_update(function(update)
    callback(update)
    if presses.stockpile == nil or presses.yield == nil or presses.elevation == nil then return end
    frames = frames + 1
    if frames % {FRAMES_BETWEEN} ~= 0 then return end
    step = step + 1
    if sequence[step] ~= nil then
      log.info('map mode press ' .. sequence[step])
      presses[sequence[step]]()
    elseif step == #sequence + 1 then
      log.info('map modes done')
      window.close()
    end
  end)
end
end
"
    )
}

/// The shipped UI script behind the wrappers.
pub(crate) fn script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(path).expect("the shipped UI script");
    format!("{}\n{main}", prefix())
}

/// Judges the combined output: the six presses reached the host in order and were each accepted, the sequence finished, nothing was
/// refused, and the window closed normally.
pub(crate) fn validate(output: &str) -> Result<(), String> {
    let lines: Vec<&str> = output.lines().collect();
    let mut start = 0;
    let mut next = |needle: &str| -> Result<(), String> {
        let Some(offset) = lines[start..].iter().position(|seen| seen.contains(needle)) else {
            return Err(format!("{needle:?} is missing or out of order"));
        };
        start += offset + 1;
        Ok(())
    };
    next("lua.ui: map mode press stockpile")?;
    next("lua.ui: set_map_mode stockpile ok")?;
    next("lua.ui: map mode press yield")?;
    next("lua.ui: set_map_mode yield ok")?;
    next("lua.ui: map mode press yield")?;
    next("lua.ui: set_map_mode off ok")?;
    next("lua.ui: map mode press stockpile")?;
    next("lua.ui: set_map_mode stockpile ok")?;
    next("lua.ui: map mode press elevation")?;
    next("lua.ui: set_map_mode elevation ok")?;
    next("lua.ui: map mode press elevation")?;
    next("lua.ui: set_map_mode off ok")?;
    next("lua.ui: map modes done")?;
    next("window: window closed")?;
    if let Some(line) = lines
        .iter()
        .find(|line| line.contains("refused") || line.contains("the map mode was refused"))
    {
        return Err(format!("a map mode or a map image was refused: {line}"));
    }
    Ok(())
}

/// The output of a good run, as the log prints it (for the offline check of the judgment).
pub(crate) const CAPTURED: &str = "INFO  lua.ui: map mode press stockpile\nINFO  lua.ui: set_map_mode stockpile ok\nINFO  lua.ui: map mode press yield\nINFO  lua.ui: set_map_mode yield ok\nINFO  lua.ui: map mode press yield\nINFO  lua.ui: set_map_mode off ok\nINFO  lua.ui: map mode press stockpile\nINFO  lua.ui: set_map_mode stockpile ok\nINFO  lua.ui: map mode press elevation\nINFO  lua.ui: set_map_mode elevation ok\nINFO  lua.ui: map mode press elevation\nINFO  lua.ui: set_map_mode off ok\nINFO  lua.ui: map modes done\nINFO  window: window closed\n";
