//! The map modes of the application (milestone 9, slice 8; `docs/economy-design.md` section 3.22): which mode shows, the table of
//! values each place has in it, and the publication of both to the map lane.
//!
//! The UI VM chooses the mode (`game.set_map_mode`, its two buttons); the values come from the economy view of the newest revision the
//! UI link has handed to the VM ([`MapModes::observe`], which the view cache calls as each view is posted). Each change of the mode, and
//! each new revision while a mode shows, publishes one immutable [`MapMode`] with a revision of its own to the scene bridge, as the
//! camera is published: the render thread reads the newest and never the live state. A mode that is off is published too, so the frame
//! returns to the plain map.

use std::sync::{Arc, Mutex, PoisonError};

use ck_render_2d::{MapMode, MapPublication, SceneBridge};

use crate::game::view::EconomyView;

/// The map mode showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The plain map.
    Off,
    /// Each place's stored goods against its capacity, red (empty) to green (full).
    Stockpile,
    /// Each place's projected annual yield against the other places', red (least) to green (most).
    Yield,
}

impl Mode {
    /// The mode a UI name stands for: `"off"`, `"stockpile"` or `"yield"`.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(Self::Off),
            "stockpile" => Some(Self::Stockpile),
            "yield" => Some(Self::Yield),
            _ => None,
        }
    }
}

struct State {
    mode: Mode,
    /// The newest view and the economy revision it came with.
    latest: Option<(u64, Arc<EconomyView>)>,
    /// The revision of the next [`MapMode`] published (they rise with each publication, so the lane never takes an old one).
    next_revision: u64,
}

/// The mode showing and the way it reaches the map lane.
pub(crate) struct MapModes {
    bridge: Arc<SceneBridge>,
    /// How many places the map's owner grid indexes: the length of every table.
    places: usize,
    state: Mutex<State>,
}

impl MapModes {
    /// The map modes of a run whose map has `places` places, published to `bridge`. The mode starts off.
    pub(crate) fn new(bridge: Arc<SceneBridge>, places: usize) -> Self {
        Self {
            bridge,
            places,
            state: Mutex::new(State {
                mode: Mode::Off,
                latest: None,
                next_revision: 1,
            }),
        }
    }

    fn locked(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The mode showing.
    #[cfg(test)]
    pub(crate) fn mode(&self) -> Mode {
        self.locked().mode
    }

    /// The table of the mode for the newest view: one value per place. Before any view every place is uncoloured.
    fn values(&self, state: &State) -> Vec<u16> {
        let from = |view: &EconomyView| match state.mode {
            Mode::Off => Vec::new(),
            Mode::Stockpile => view.stockpile_values(),
            Mode::Yield => view.yield_values(),
        };
        match &state.latest {
            Some((_, view)) => from(view),
            None => vec![ck_render_2d::MODE_UNCOLOURED; self.places],
        }
    }

    /// Publishes the mode and its values as the next revision. The lane's refusal (a table that is not one value per place) is the
    /// error.
    fn publish(&self, state: &mut State) -> Result<(), String> {
        let revision = state.next_revision;
        let mode = if state.mode == Mode::Off {
            MapMode::off(revision)
        } else {
            MapMode::new(revision, self.values(state))
                .map_err(|error| format!("the map mode was refused: {error}"))?
        };
        match self.bridge.publish_mode(Arc::new(mode)) {
            Ok(MapPublication::Published) => {
                state.next_revision += 1;
                Ok(())
            }
            // A detached bridge takes nothing more: the run is ending.
            Ok(MapPublication::Detached) => Ok(()),
            Ok(MapPublication::Stale) => Err("the map mode was refused: stale revision".to_owned()),
            Err(error) => Err(format!("the map mode was refused: {error}")),
        }
    }

    /// Selects `mode` and publishes it. Selecting the mode that shows publishes nothing.
    ///
    /// # Errors
    ///
    /// The lane's refusal, named. The mode that showed before stays.
    pub(crate) fn set(&self, mode: Mode) -> Result<(), String> {
        let mut state = self.locked();
        if state.mode == mode {
            return Ok(());
        }
        let before = state.mode;
        state.mode = mode;
        let published = self.publish(&mut state);
        if published.is_err() {
            state.mode = before;
        }
        published
    }

    /// The UI link handed the VM the view of economy revision `revision`. A view the modes already hold does nothing; a new one is the
    /// values of a mode that shows, published again.
    pub(crate) fn observe(&self, revision: u64, view: &Arc<EconomyView>) {
        let mut state = self.locked();
        if state
            .latest
            .as_ref()
            .is_some_and(|(held, _)| *held >= revision)
        {
            return;
        }
        state.latest = Some((revision, Arc::clone(view)));
        if state.mode == Mode::Off {
            return;
        }
        if let Err(message) = self.publish(&mut state) {
            let _ = ck_log::try_log!(ck_log::Level::Warn, "map", "{message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ck_render_2d::{Filter, QuadSink, Snapshot};
    use ck_window::{PixelSize, WindowDesc};

    use super::*;
    use crate::game::view::PlaceView;
    use crate::map_app::MapController;

    /// The latest snapshot the bridge published and how many it published.
    #[derive(Default)]
    struct Sink {
        latest: Mutex<Option<Snapshot>>,
        count: AtomicUsize,
    }

    impl QuadSink for Sink {
        fn publish(&self, snapshot: Snapshot) -> Option<Snapshot> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.latest
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .replace(snapshot)
        }
    }

    impl Sink {
        fn mode(&self) -> Option<(u64, Vec<u16>)> {
            let latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
            let map = latest
                .as_ref()
                .expect("published")
                .map
                .as_ref()
                .expect("a map");
            map.mode
                .as_ref()
                .and_then(|mode| Some((mode.revision(), mode.values()?.to_vec())))
        }

        fn published(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    const PLACES: usize = 94;

    /// The shipped Ireland scenario's map on a bridge with a sink attached, and its modes.
    fn rig() -> (Arc<MapModes>, Arc<Sink>, ck_map::Scenario) {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768");
        let scenario = ck_map::load_scenario(&directory).expect("the scenario loads");
        assert_eq!(scenario.locations.len(), PLACES);
        let bridge = SceneBridge::new(Filter::Nearest);
        let mut controller =
            MapController::from_scenario(&scenario, Arc::clone(&bridge), 1, 20, 24)
                .expect("the map lane takes the scenario");
        let desc = WindowDesc {
            title: "map modes".to_owned(),
            width: 400,
            height: 320,
            clear: [0.0; 3],
        };
        controller.open(&desc).expect("the initial view is valid");
        let sink = Arc::new(Sink::default());
        bridge
            .attach(
                &desc,
                PixelSize {
                    width: 800,
                    height: 640,
                },
                sink.clone(),
            )
            .expect("the sink attaches");
        (Arc::new(MapModes::new(bridge, PLACES)), sink, scenario)
    }

    /// A view of 94 places: place `i` has `i * 10` of 1,000 stored and a yield of `i`, and every fifth has no economy.
    fn view(shift: u64) -> Arc<EconomyView> {
        Arc::new(EconomyView {
            places: (0..PLACES as u64)
                .map(|index| PlaceView {
                    id: format!("place-{index}"),
                    storage_used_ml: (index * 10 + shift).min(1_000),
                    storage_capacity_ml: if index % 5 == 4 { 0 } else { 1_000 },
                    projected_yield: index + shift,
                    has_economy: index % 5 != 4,
                    ..PlaceView::default()
                })
                .collect(),
        })
    }

    #[test]
    fn the_scenario_map_carries_its_owner_grid_over_its_places() {
        let (_modes, sink, scenario) = rig();
        let latest = sink.latest.lock().unwrap();
        let map = latest
            .as_ref()
            .expect("published")
            .map
            .as_ref()
            .expect("a map");
        let owners = map.input.owners().expect("the map has an owner grid");
        assert_eq!(owners.places(), PLACES);
        assert_eq!(owners.indices().len(), scenario.catchment_owners.len());
        for (index, (given, owner)) in owners
            .indices()
            .iter()
            .zip(&scenario.catchment_owners)
            .enumerate()
        {
            assert_eq!(
                *given,
                owner.map_or(ck_render_2d::MODE_UNOWNED, |place| u16::try_from(place)
                    .unwrap()),
                "cell {index}"
            );
        }
        assert!(
            owners
                .indices()
                .iter()
                .any(|owner| *owner != ck_render_2d::MODE_UNOWNED),
            "some cells are owned"
        );
        assert!(map.mode.is_none(), "no mode shows at the start");
    }

    #[test]
    fn a_mode_publishes_the_values_of_the_newest_view_and_off_returns_the_plain_map() {
        let (modes, sink, _) = rig();
        let first = view(0);
        modes.observe(1, &first);
        assert_eq!(sink.mode(), None, "a view alone shows nothing");
        modes.set(Mode::Stockpile).expect("accepted");
        let (stockpile_revision, values) = sink.mode().expect("a mode shows");
        assert_eq!(values, first.stockpile_values());
        assert_eq!(values[3], 30);
        assert_eq!(
            values[4],
            ck_render_2d::MODE_UNCOLOURED,
            "no capacity: uncoloured"
        );
        modes.set(Mode::Yield).expect("accepted");
        let (yield_revision, values) = sink.mode().expect("a mode shows");
        assert_eq!(values, first.yield_values());
        assert!(
            yield_revision > stockpile_revision,
            "each publication has its own revision"
        );
        assert_eq!(values[4], ck_render_2d::MODE_UNCOLOURED);
        assert_eq!(values[0], 0);
        modes.set(Mode::Off).expect("accepted");
        assert_eq!(sink.mode(), None, "off is the plain map");
        assert_eq!(modes.mode(), Mode::Off);
    }

    #[test]
    fn a_new_revision_republishes_a_mode_that_shows_and_nothing_else_does() {
        let (modes, sink, _) = rig();
        modes.observe(1, &view(0));
        let before = sink.published();
        modes.observe(2, &view(100));
        assert_eq!(
            sink.published(),
            before,
            "off: a new view is held and publishes nothing"
        );
        modes.set(Mode::Stockpile).expect("accepted");
        let after_set = sink.published();
        assert_eq!(
            sink.mode().expect("shows").1,
            view(100).stockpile_values(),
            "the newest view's values"
        );
        // The mode that shows again, and a revision already seen, publish nothing.
        modes.set(Mode::Stockpile).expect("accepted");
        modes.observe(2, &view(100));
        modes.observe(1, &view(0));
        assert_eq!(sink.published(), after_set);
        // A newer revision republishes with new values, under a newer mode revision.
        let (revision, _) = sink.mode().expect("shows");
        modes.observe(3, &view(500));
        let (newer, values) = sink.mode().expect("shows");
        assert!(newer > revision);
        assert_eq!(values, view(500).stockpile_values());
        assert_eq!(sink.published(), after_set + 1);
    }

    #[test]
    fn before_any_view_a_mode_shows_every_place_uncoloured() {
        let (modes, sink, _) = rig();
        modes.set(Mode::Yield).expect("accepted");
        assert_eq!(
            sink.mode().expect("shows").1,
            vec![ck_render_2d::MODE_UNCOLOURED; PLACES]
        );
    }

    #[test]
    fn a_table_the_lane_refuses_is_an_error_and_the_mode_that_showed_stays() {
        let (modes, sink, _) = rig();
        // A view for the wrong number of places (a scenario that does not match its map) is refused by the lane, named.
        let wrong = Arc::new(EconomyView {
            places: vec![PlaceView::default(); PLACES - 1],
        });
        modes.observe(1, &wrong);
        let before = sink.published();
        let error = modes.set(Mode::Stockpile).expect_err("refused");
        assert!(
            error.contains("93 values") && error.contains("94 places"),
            "{error}"
        );
        assert_eq!(modes.mode(), Mode::Off, "the mode that showed stays");
        assert_eq!(sink.published(), before, "a refusal publishes nothing");
        // Off is never refused.
        modes.set(Mode::Off).expect("off");
        // The error reaches Lua by name, and an unknown name is refused.
        let function = crate::game_api::set_map_mode_function(Arc::clone(&modes));
        let refused = function(&[Some(ck_lua::ConfigValue::String("stockpile".to_owned()))])
            .expect_err("refused");
        assert!(
            refused.starts_with("game.set_map_mode: ") && refused.contains("93 values"),
            "{refused}"
        );
        let unknown = function(&[Some(ck_lua::ConfigValue::String("rainfall".to_owned()))])
            .expect_err("refused");
        assert!(unknown.contains("unknown mode `rainfall`"), "{unknown}");
        assert!(function(&[]).is_err());
    }
}
