//! The application's single owner of the map camera and its paced keyboard controls.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ck_map::look::TerrainShades;
use ck_map::{
    Camera, Crop, FixedPoint, LoadedMap, Location, LocationKinds, PreparedLocations,
    PreparedMarkers, PreparedRivers, STEP_SIZE, Scenario, TerrainKind, ViewSize, WorldPixel,
};
use ck_render_2d::{
    MapCamera, MapInput, MapPublication, MapRules, MapTerrainKind, MapTerrainRule, SceneBridge,
};
use ck_window::{ActionSnapshot, InputTick, PointerEvent, PointerPosition, ScrollUnit, WindowDesc};

const MAP_INPUT_GENERATION: u64 = 1;
const PAN_LEFT: &str = "map_pan_left";
const PAN_RIGHT: &str = "map_pan_right";
const PAN_UP: &str = "map_pan_up";
const PAN_DOWN: &str = "map_pan_down";
const ZOOM_IN: &str = "map_zoom_in";
const ZOOM_OUT: &str = "map_zoom_out";

#[derive(Clone, Copy)]
struct MapControls {
    pan_pixels_per_frame: u32,
    zoom_steps_per_second: u32,
    scroll_points_per_step: u32,
}

pub(crate) type SharedController = Arc<Mutex<MapController>>;

/// The current marker hover and its outer horizontal bounds in window points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hover {
    pub(crate) location_index: usize,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) left: i64,
    pub(crate) right: i64,
    pub(crate) y: i64,
}

/// A selected place, delivered to the UI once per selecting click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) seq: u64,
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) size: u8,
}

pub(crate) trait RiverViewSelector {
    fn select_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
    ) -> Option<Vec<[u32; 2]>>;
}

impl RiverViewSelector for PreparedRivers {
    fn select_view(
        &mut self,
        step: u8,
        top_left: WorldPixel,
        view: ViewSize,
    ) -> Option<Vec<[u32; 2]>> {
        PreparedRivers::select_view(self, step, top_left, view)
    }
}

/// A map, its packed GPU input, and the one camera controlled by the application.
pub(crate) struct MapController<R: RiverViewSelector = PreparedRivers> {
    bridge: Arc<SceneBridge>,
    crop: Crop,
    rivers: R,
    locations: PreparedLocations,
    markers: PreparedMarkers,
    location_names: Vec<(String, String)>,
    location_details: Vec<(String, u8)>,
    selection: Option<Selection>,
    pointer_position: Option<PointerPosition>,
    hover: Option<Hover>,
    overlay_palette: Vec<ck_map::Rgba8>,
    pan_pixels_per_frame: i32,
    zoom_steps_per_second: u32,
    scroll_points_per_step: u32,
    drag: Option<Drag>,
    scroll_remainder: [i32; 2],
    scroll_direction: [i8; 2],
    camera: Option<Camera>,
    /// The scene bridge's metrics generation the camera was last published (or staged) for. A newer generation (the window's points
    /// or pixels changed, even when the view, pan and zoom did not: a change of display scale moves only the pixels) means the
    /// bridge refuses the camera as stale, so it is published again for the current one.
    published_generation: u64,
    zoom_in_elapsed: Duration,
    zoom_out_elapsed: Duration,
}

pub(crate) enum RoutedMapEvent {
    Pointer(PointerEvent),
    EndDrag,
}

#[derive(Clone, Copy)]
struct Drag {
    anchor: PointerPosition,
    applied: (i32, i32),
    dragged: bool,
    eligible: bool,
}

impl MapController<PreparedRivers> {
    pub(crate) fn new(
        loaded: &LoadedMap,
        bridge: Arc<SceneBridge>,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
        scroll_points_per_step: u32,
    ) -> Result<Self, String> {
        Self::with_scenario_parts(
            loaded,
            bridge,
            MapControls {
                pan_pixels_per_frame,
                zoom_steps_per_second,
                scroll_points_per_step,
            },
            PreparedRivers::new(loaded.rivers.clone()),
            Vec::new(),
            LocationKinds::default(),
            &[],
        )
    }

    pub(crate) fn from_scenario(
        scenario: &Scenario,
        bridge: Arc<SceneBridge>,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
        scroll_points_per_step: u32,
    ) -> Result<Self, String> {
        Self::with_scenario_parts(
            &scenario.map,
            bridge,
            MapControls {
                pan_pixels_per_frame,
                zoom_steps_per_second,
                scroll_points_per_step,
            },
            PreparedRivers::new(scenario.map.rivers.clone()),
            scenario.locations.clone(),
            scenario.location_kinds.clone(),
            &scenario.catchment_owners,
        )
    }
}

impl<R: RiverViewSelector> MapController<R> {
    /// Changes all three input rates together; the next tick observes the new values.
    pub(crate) fn set_controls(
        &mut self,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
        scroll_points_per_step: u32,
    ) {
        self.pan_pixels_per_frame =
            i32::try_from(pan_pixels_per_frame).expect("validated map pan range fits i32");
        self.zoom_steps_per_second = zoom_steps_per_second;
        self.scroll_points_per_step = scroll_points_per_step;
    }

    #[cfg(test)]
    fn with_rivers(
        loaded: &LoadedMap,
        bridge: Arc<SceneBridge>,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
        scroll_points_per_step: u32,
        rivers: R,
    ) -> Result<Self, String> {
        Self::with_scenario_parts(
            loaded,
            bridge,
            MapControls {
                pan_pixels_per_frame,
                zoom_steps_per_second,
                scroll_points_per_step,
            },
            rivers,
            Vec::new(),
            LocationKinds::default(),
            &[],
        )
    }

    /// `owners` is the scenario's catchment owner of every crop cell (a place's index in `locations`, or none), empty for a map with
    /// no places: the owner grid the map modes read goes to the lane with the map.
    fn with_scenario_parts(
        loaded: &LoadedMap,
        bridge: Arc<SceneBridge>,
        controls: MapControls,
        rivers: R,
        locations: Vec<Location>,
        kinds: LocationKinds,
        owners: &[Option<usize>],
    ) -> Result<Self, String> {
        let crop = loaded.data.crop;
        let locations_count = locations.len();
        let overlay_palette = kinds.overlay_palette();
        let markers = PreparedMarkers::new(locations.clone(), kinds.clone());
        let location_names = locations
            .iter()
            .map(|location| (location.id.clone(), location.name.clone()))
            .collect();
        let location_details = locations
            .iter()
            .map(|location| (kinds.kinds[location.kind_index].id.clone(), location.size))
            .collect();
        let location_cache = PreparedLocations::new(locations, kinds, crop);
        let rules = rules(loaded, &overlay_palette);
        let input = MapInput::new(
            MAP_INPUT_GENERATION,
            [crop.origin.x, crop.origin.y],
            [crop.width, crop.height],
            flatten(&loaded.grid_a.pixels),
            flatten(&loaded.grid_b.pixels),
            rules,
        )
        .map_err(|error| format!("map lane refused the loaded map: {error}"))?;
        let places = locations_count;
        let input = if owners.is_empty() {
            input
        } else {
            let owners = owners
                .iter()
                .map(|owner| {
                    owner.map_or(Ok(ck_render_2d::MODE_UNOWNED), |index| {
                        u16::try_from(index)
                            .map_err(|_| format!("place index {index} does not fit the owner grid"))
                    })
                })
                .collect::<Result<Vec<u16>, String>>()?;
            input
                .with_owners(owners, places)
                .map_err(|error| format!("map lane refused the owner grid: {error}"))?
        };
        let input = Arc::new(input);
        match bridge.publish_map(Arc::clone(&input)) {
            MapPublication::Published => {}
            outcome => return Err(format!("map lane refused the loaded map: {outcome:?}")),
        }
        Ok(Self {
            bridge,
            crop,
            rivers,
            locations: location_cache,
            markers,
            location_names,
            location_details,
            selection: None,
            pointer_position: None,
            hover: None,
            overlay_palette,
            pan_pixels_per_frame: i32::try_from(controls.pan_pixels_per_frame)
                .map_err(|_| "map pan_pixels_per_frame does not fit i32".to_owned())?,
            zoom_steps_per_second: controls.zoom_steps_per_second,
            scroll_points_per_step: controls.scroll_points_per_step,
            drag: None,
            scroll_remainder: [0; 2],
            scroll_direction: [0; 2],
            camera: None,
            published_generation: 0,
            zoom_in_elapsed: Duration::ZERO,
            zoom_out_elapsed: Duration::ZERO,
        })
    }

    /// Set up the initial view from the exact window description before the request reaches the backend.
    pub(crate) fn open(&mut self, desc: &WindowDesc) -> Result<(), String> {
        let view = ViewSize::from_points(desc.width, desc.height);
        let camera = initial_camera(self.crop, view)?;
        let generation = self.bridge.metrics_generation().saturating_add(1);
        let snapshot = self.snapshot(camera, generation)?;
        self.camera = Some(camera);
        self.published_generation = generation;
        #[cfg(feature = "test-support")]
        Self::observe_test_camera(camera)?;
        match self.bridge.stage_initial_camera(Arc::new(snapshot)) {
            MapPublication::Published => Ok(()),
            outcome => Err(format!("map lane refused the initial camera: {outcome:?}")),
        }
    }

    /// Apply one paced input snapshot, publishing only when the camera or metrics changed.
    #[cfg(test)]
    pub(crate) fn update(&mut self, update: &InputTick) -> Result<bool, String> {
        let events = update
            .input
            .pointer_events
            .iter()
            .copied()
            .map(RoutedMapEvent::Pointer)
            .collect::<Vec<_>>();
        self.update_routed(
            update,
            &events,
            update.input.pointer_position,
            update.input.reset,
            update.input.pointer_overflow,
        )
    }

    /// Applies the map's ordered share of a tick. `EndDrag` is an ownership boundary and is replayed
    /// at its event position, before any later retained pointer event.
    #[expect(
        clippy::too_many_lines,
        reason = "routed pointer replay, keyboard control, and one publication form one ordered update"
    )]
    pub(crate) fn update_routed(
        &mut self,
        update: &InputTick,
        events: &[RoutedMapEvent],
        hover_position: Option<PointerPosition>,
        suppress_hover: bool,
        raw_overflow: bool,
    ) -> Result<bool, String> {
        let Some(mut camera) = self.camera else {
            return Ok(false);
        };
        #[cfg(feature = "test-support")]
        let camera_before_input = camera;
        let view = ViewSize::from_points(update.metrics.points.0, update.metrics.points.1);
        let mut changed = false;
        if camera.view != view {
            camera.resize(view);
            changed = true;
        }

        // Pointer gestures are replayed before keyboard state, preserving the input contract.
        if raw_overflow && let Some(drag) = &mut self.drag {
            drag.eligible = false;
        }
        for routed in events {
            let RoutedMapEvent::Pointer(event) = routed else {
                self.end_drag();
                continue;
            };
            #[cfg(feature = "test-support")]
            Self::observe_test_pointer(*event)?;
            match *event {
                PointerEvent::Down(position) => {
                    self.drag = Some(Drag {
                        anchor: position,
                        applied: (0, 0),
                        dragged: false,
                        eligible: !raw_overflow,
                    });
                }
                PointerEvent::ButtonDown { .. } | PointerEvent::ButtonUp { .. } => {}
                PointerEvent::Move(position) | PointerEvent::Up(position) => {
                    if let Some(drag) = &mut self.drag {
                        let total = drag_delta(drag.anchor, position);
                        drag.dragged |= total != (0, 0);
                        let delta = (total.0 - drag.applied.0, total.1 - drag.applied.1);
                        if delta != (0, 0) {
                            let before = camera;
                            camera.pan(delta.0, delta.1, self.crop);
                            changed |= camera != before;
                        }
                        drag.applied = total;
                        if matches!(*event, PointerEvent::Up(_)) {
                            let click = drag.eligible && !drag.dragged;
                            if click && let Some(selected) = self.selected_at(camera, position) {
                                self.selection = Some(selected);
                            }
                        }
                    }
                    if matches!(*event, PointerEvent::Up(_)) {
                        self.drag = None;
                    }
                }
                PointerEvent::Scroll {
                    position,
                    amount,
                    unit,
                } => {
                    let index = usize::from(unit == ScrollUnit::Points);
                    let direction = amount.signum() as i8;
                    if direction != 0
                        && self.scroll_direction[index] != 0
                        && direction != self.scroll_direction[index]
                    {
                        self.scroll_remainder[index] = 0;
                    }
                    if direction != 0 {
                        self.scroll_direction[index] = direction;
                    }
                    let threshold = match unit {
                        ScrollUnit::Lines => 256_i32,
                        ScrollUnit::Points => {
                            i32::try_from(self.scroll_points_per_step.saturating_mul(256))
                                .unwrap_or(i32::MAX)
                        }
                    };
                    let total = self.scroll_remainder[index].saturating_add(amount);
                    let steps = total / threshold;
                    self.scroll_remainder[index] = total % threshold;
                    for _ in 0..steps.unsigned_abs().min(75) {
                        changed |= zoom_at(&mut camera, steps.signum() as i16, position, self.crop);
                    }
                }
            }
        }
        if update.input.pointer_overflow {
            if let (Some(drag), Some(position)) = (&mut self.drag, update.input.pointer_position) {
                let total = drag_delta(drag.anchor, position);
                let delta = (total.0 - drag.applied.0, total.1 - drag.applied.1);
                if delta != (0, 0) {
                    camera.pan(delta.0, delta.1, self.crop);
                    changed = true;
                }
                drag.applied = total;
            }
            if !update.input.pointer_held {
                self.drag = None;
            }
        }

        let actions = &update.input.actions;
        let left = held(actions, PAN_LEFT);
        let right = held(actions, PAN_RIGHT);
        let up = held(actions, PAN_UP);
        let down = held(actions, PAN_DOWN);
        let dx = i32::from(right) - i32::from(left);
        let dy = i32::from(down) - i32::from(up);
        if dx != 0 || dy != 0 {
            let before = camera;
            camera.pan(
                dx * self.pan_pixels_per_frame,
                dy * self.pan_pixels_per_frame,
                self.crop,
            );
            changed |= before != camera;
        }

        let zoom_in = action(actions, ZOOM_IN);
        let zoom_out = action(actions, ZOOM_OUT);
        let direction = match (
            zoom_in.is_some_and(|item| item.held),
            zoom_out.is_some_and(|item| item.held),
        ) {
            (true, false) => Some((1_i16, zoom_in.expect("held action exists"))),
            (false, true) => Some((-1_i16, zoom_out.expect("held action exists"))),
            _ => None,
        };
        if let Some((direction, state)) = direction {
            let elapsed = if direction > 0 {
                &mut self.zoom_in_elapsed
            } else {
                &mut self.zoom_out_elapsed
            };
            if state.pressed {
                changed |= change_step(&mut camera, direction);
                *elapsed = Duration::ZERO;
            } else {
                *elapsed = elapsed.saturating_add(update.dt);
                let interval = Duration::from_secs_f64(1.0 / f64::from(self.zoom_steps_per_second));
                while *elapsed >= interval {
                    *elapsed -= interval;
                    changed |= change_step(&mut camera, direction);
                }
            }
        } else {
            self.zoom_in_elapsed = Duration::ZERO;
            self.zoom_out_elapsed = Duration::ZERO;
        }

        self.pointer_position = update.input.pointer_position;
        self.hover = if update.input.reset || update.input.pointer_held || suppress_hover {
            None
        } else {
            hover_position
                .filter(|position| pointer_in_view(*position, update.metrics.points))
                .and_then(|position| {
                    hover_at(&mut self.markers, &self.location_names, camera, position)
                })
        };

        // The bridge's metrics are a newer generation than the camera's last publication: the camera is published again for it, even
        // when nothing about the view changed.
        let generation = self.bridge.metrics_generation();
        if changed || generation > self.published_generation {
            #[cfg(feature = "test-support")]
            if camera != camera_before_input {
                Self::observe_test_camera(camera)?;
            }
            self.camera = Some(camera);
            self.publish_for_generation(camera, generation)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Ends a map-owned gesture without applying any movement to the next pointer position.
    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
    }

    #[cfg(feature = "test-support")]
    fn observe_test_camera(camera: Camera) -> Result<(), String> {
        Self::append_test_observation(&format!(
            "camera {} {} {}",
            camera.step, camera.center.x, camera.center.y
        ))
    }

    #[cfg(feature = "test-support")]
    fn observe_test_pointer(event: PointerEvent) -> Result<(), String> {
        let kind = match event {
            PointerEvent::Down(_) => "down",
            PointerEvent::Move(_) => "move",
            PointerEvent::Up(_) => "up",
            PointerEvent::ButtonDown {
                button: ck_window::PointerButton::Middle,
                ..
            } => "button-down-middle",
            PointerEvent::ButtonDown {
                button: ck_window::PointerButton::Right,
                ..
            } => "button-down-right",
            PointerEvent::ButtonUp {
                button: ck_window::PointerButton::Middle,
                ..
            } => "button-up-middle",
            PointerEvent::ButtonUp {
                button: ck_window::PointerButton::Right,
                ..
            } => "button-up-right",
            PointerEvent::Scroll {
                unit: ScrollUnit::Lines,
                ..
            } => "scroll-lines",
            PointerEvent::Scroll {
                unit: ScrollUnit::Points,
                ..
            } => "scroll-points",
        };
        Self::append_test_observation(&format!("pointer {kind}"))
    }

    #[cfg(feature = "test-support")]
    fn append_test_observation(line: &str) -> Result<(), String> {
        use std::io::Write as _;

        let Some(path) = std::env::var_os("CK_MAP_TEST_CAMERA_OBSERVATION") else {
            return Ok(());
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| format!("cannot open map test observation: {error}"))?;
        writeln!(file, "{line}")
            .map_err(|error| format!("cannot write map test observation: {error}"))
    }

    fn snapshot(&mut self, camera: Camera, generation: u64) -> Result<MapCamera, String> {
        let top_left = camera.top_left();
        let rivers = self
            .rivers
            .select_view(camera.step, top_left, camera.view)
            .ok_or_else(|| format!("map river coverage is invalid for step {}", camera.step))?;
        let locations = self
            .locations
            .select_view(camera.step, top_left, camera.view)
            .ok_or_else(|| format!("map location pixels are invalid for step {}", camera.step))?
            .into_iter()
            .map(|(xy, index)| [xy[0], xy[1], u32::from(index)])
            .collect();
        MapCamera::new_with_locations(
            camera.step,
            [top_left.x, top_left.y],
            [camera.view.width, camera.view.height],
            rivers,
            locations,
            self.overlay_palette.len(),
            generation,
        )
        .map_err(|error| format!("map camera is invalid: {error}"))
    }

    fn publish_for_generation(&mut self, camera: Camera, generation: u64) -> Result<(), String> {
        let snapshot = self.snapshot(camera, generation)?;
        match self.bridge.publish_camera(Arc::new(snapshot)) {
            MapPublication::Published => {
                self.published_generation = generation;
                Ok(())
            }
            outcome => Err(format!("map lane refused the camera snapshot: {outcome:?}")),
        }
    }

    #[cfg(test)]
    pub(crate) fn camera(&self) -> Option<Camera> {
        self.camera
    }

    pub(crate) fn hover(&self) -> Option<Hover> {
        self.hover.clone()
    }

    pub(crate) fn selection(&self) -> Option<Selection> {
        self.selection.clone()
    }

    fn selected_at(&mut self, camera: Camera, position: PointerPosition) -> Option<Selection> {
        let hover = hover_at(&mut self.markers, &self.location_names, camera, position)?;
        if camera.step < 15 {
            return None;
        }
        let (kind, size) = self.location_details.get(hover.location_index)?.clone();
        let seq = self
            .selection
            .as_ref()
            .map_or(1, |selection| selection.seq.saturating_add(1));
        Some(Selection {
            seq,
            id: hover.id,
            name: hover.name,
            kind,
            size,
        })
    }

    pub(crate) fn hit_regions(&self) -> Option<Arc<ck_render_2d::HitRegions>> {
        self.bridge.hit_regions()
    }
}

fn pointer_in_view(position: PointerPosition, points: (u32, u32)) -> bool {
    position.x >= 0
        && position.y >= 0
        && i64::from(position.x) < i64::from(points.0) * 256
        && i64::from(position.y) < i64::from(points.1) * 256
}

fn hover_at(
    markers: &mut PreparedMarkers,
    names: &[(String, String)],
    camera: Camera,
    position: PointerPosition,
) -> Option<Hover> {
    let map_x = i64::from(position.x).div_euclid(4 * 256);
    let map_y = i64::from(position.y).div_euclid(4 * 256);
    let top_left = camera.top_left();
    let pixel = WorldPixel {
        x: i32::try_from(i64::from(top_left.x) + map_x).ok()?,
        y: i32::try_from(i64::from(top_left.y) + map_y).ok()?,
    };
    let index = markers.hit(camera.step, pixel)?;
    let (id, name) = names.get(index)?;
    let (marker_top_left, marker_bottom_right) = markers.hit_bounds(camera.step, index)?;
    let left = (i64::from(marker_top_left.x) - i64::from(top_left.x)) * 4;
    let right = (i64::from(marker_bottom_right.x) + 1 - i64::from(top_left.x)) * 4;
    let y = (i64::from(marker_top_left.y) + i64::from(marker_bottom_right.y) + 1
        - 2 * i64::from(top_left.y))
        * 2;
    Some(Hover {
        location_index: index,
        id: id.clone(),
        name: name.clone(),
        left,
        right,
        y,
    })
}

pub(crate) fn shared(controller: MapController) -> SharedController {
    Arc::new(Mutex::new(controller))
}

pub(crate) fn lock(controller: &SharedController) -> std::sync::MutexGuard<'_, MapController> {
    controller.lock().unwrap_or_else(PoisonError::into_inner)
}

fn action<'a>(actions: &'a [ActionSnapshot], name: &str) -> Option<&'a ActionSnapshot> {
    actions.iter().find(|item| item.name == name)
}

fn held(actions: &[ActionSnapshot], name: &str) -> bool {
    action(actions, name).is_some_and(|item| item.held)
}

fn change_step(camera: &mut Camera, direction: i16) -> bool {
    let step = (i16::from(camera.step) + direction).clamp(0, 75);
    let step = u8::try_from(step).expect("step is clamped to the u8 range");
    step != camera.step && camera.set_step(step)
}

fn drag_delta(anchor: PointerPosition, position: PointerPosition) -> (i32, i32) {
    let divisor = 4 * 256;
    let dx = (i64::from(position.x) - i64::from(anchor.x)).div_euclid(divisor);
    let dy = (i64::from(position.y) - i64::from(anchor.y)).div_euclid(divisor);
    (
        i32::try_from(-dx).unwrap_or(i32::MIN),
        i32::try_from(-dy).unwrap_or(i32::MIN),
    )
}

fn zoom_at(camera: &mut Camera, direction: i16, position: PointerPosition, crop: Crop) -> bool {
    let next = u8::try_from((i16::from(camera.step) + direction).clamp(0, 75))
        .expect("zoom step is clamped to u8 range");
    if next == camera.step {
        return false;
    }
    let old_size = i64::from(STEP_SIZE[usize::from(camera.step)]);
    let new_size = i64::from(STEP_SIZE[usize::from(next)]);
    let cursor_x = i64::from(position.x).div_euclid(4 * 256);
    let cursor_y = i64::from(position.y).div_euclid(4 * 256);
    let top = camera.top_left();
    let world_x = (i64::from(top.x) + cursor_x) * old_size + old_size / 2;
    let world_y = (i64::from(top.y) + cursor_y) * old_size + old_size / 2;
    let left = world_x.div_euclid(new_size) - cursor_x;
    let upper = world_y.div_euclid(new_size) - cursor_y;
    let center = FixedPoint {
        x: i32::try_from((left + i64::from(camera.view.width / 2)) * new_size + new_size / 2)
            .unwrap_or(i32::MAX),
        y: i32::try_from((upper + i64::from(camera.view.height / 2)) * new_size + new_size / 2)
            .unwrap_or(i32::MAX),
    };
    let Some(anchored) = Camera::new(next, center, camera.view, crop) else {
        return false;
    };
    *camera = anchored;
    true
}

fn initial_step(crop: Crop, view: ViewSize) -> u8 {
    let fits = |step: u8| {
        let pixel_size = i64::from(STEP_SIZE[usize::from(step)]);
        i64::from(crop.width) * ck_map::FIXED_PER_CELL <= i64::from(view.width) * pixel_size
            && i64::from(crop.height) * ck_map::FIXED_PER_CELL
                <= i64::from(view.height) * pixel_size
    };
    (0..=75).find(|step| fits(*step)).unwrap_or(0)
}

fn initial_camera(crop: Crop, view: ViewSize) -> Result<Camera, String> {
    let center = FixedPoint {
        x: i32::try_from(
            i64::from(crop.origin.x) * ck_map::FIXED_PER_CELL
                + i64::from(crop.width) * ck_map::FIXED_PER_CELL / 2,
        )
        .map_err(|_| "map crop centre is outside fixed-point coordinates".to_owned())?,
        y: i32::try_from(
            i64::from(crop.origin.y) * ck_map::FIXED_PER_CELL
                + i64::from(crop.height) * ck_map::FIXED_PER_CELL / 2,
        )
        .map_err(|_| "map crop centre is outside fixed-point coordinates".to_owned())?,
    };
    Camera::new(initial_step(crop, view), center, view, crop)
        .ok_or_else(|| "map camera refused the initial view".to_owned())
}

fn flatten(pixels: &[[u8; 4]]) -> Vec<u8> {
    pixels
        .iter()
        .flat_map(|pixel| pixel.iter().copied())
        .collect()
}

fn rgba(colour: ck_map::Rgba8) -> [u8; 4] {
    colour.0
}

fn rules(loaded: &LoadedMap, overlay_palette: &[ck_map::Rgba8]) -> MapRules {
    let terrains = loaded
        .terrain_types
        .iter()
        .map(|terrain| {
            let palette = rgba(terrain.palette_colour);
            let style = terrain
                .style_index
                .map(|index| &loaded.data.terrains[usize::from(index)]);
            // The ten tones of a land type's four colours (computed here, once per load, by the reference's own function); a type with no
            // style shows its palette colour in every tone.
            let (tone, shadow, lit) =
                style.map_or(([palette; 4], [palette; 3], [palette; 3]), |style| {
                    let shades = TerrainShades::of(style);
                    (
                        shades.tone.map(rgba),
                        shades.shadow.map(rgba),
                        shades.lit.map(rgba),
                    )
                });
            let kind = match terrain.kind {
                TerrainKind::Land => MapTerrainKind::Land,
                TerrainKind::Sea => MapTerrainKind::Sea,
                TerrainKind::Lake => MapTerrainKind::Lake,
                TerrainKind::Mountain => MapTerrainKind::Mountain,
            };
            MapTerrainRule {
                kind,
                tone,
                shadow,
                lit,
            }
        })
        .collect();
    MapRules {
        terrains,
        sea: rgba(loaded.data.sea),
        shallow_sea: rgba(loaded.data.shallow_sea),
        lake: rgba(loaded.data.lake),
        river: rgba(loaded.data.river),
        ice: rgba(loaded.data.ice),
        ice_second: rgba(loaded.data.ice_second),
        overlay_palette: overlay_palette.iter().map(|colour| rgba(*colour)).collect(),
        shallow_threshold: loaded.data.shallow_threshold,
        steps: STEP_SIZE,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use ck_lua::{ConfigValue, Extension};
    use ck_window::{InputSnapshot, PixelSize, WindowMetrics};

    #[derive(Default)]
    struct CapturingSink(Mutex<Option<ck_render_2d::Snapshot>>);

    impl ck_render_2d::QuadSink for CapturingSink {
        fn publish(&self, snapshot: ck_render_2d::Snapshot) -> Option<ck_render_2d::Snapshot> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .replace(snapshot)
        }
    }

    impl CapturingSink {
        fn latest(&self) -> ck_render_2d::Snapshot {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .expect("bridge published a snapshot")
                .clone()
        }
    }

    struct CountingRivers {
        prepared: PreparedRivers,
        prepared_steps: HashSet<u8>,
        preparations: usize,
    }

    impl CountingRivers {
        fn new(lines: Vec<ck_map::RiverLine>) -> Self {
            Self {
                prepared: PreparedRivers::new(lines),
                prepared_steps: HashSet::new(),
                preparations: 0,
            }
        }
    }

    impl RiverViewSelector for CountingRivers {
        fn select_view(
            &mut self,
            step: u8,
            top_left: WorldPixel,
            view: ViewSize,
        ) -> Option<Vec<[u32; 2]>> {
            if self.prepared_steps.insert(step) {
                self.preparations += 1;
            }
            self.prepared.select_view(step, top_left, view)
        }
    }

    fn loaded_map() -> LoadedMap {
        let crop = Crop {
            origin: ck_map::Cell {
                x: 23_882,
                y: 23_882,
            },
            width: 2,
            height: 2,
        };
        let water = ck_map::Rgba8::opaque(5, 8, 14);
        LoadedMap {
            data: ck_map::MapData {
                crop,
                cells: vec![ck_map::Surface::Sea { depth: 0 }; 4],
                elevations: vec![11_000; 4],
                fertility: vec![0; 4],
                climate_temperature_tenths: vec![100; 4],
                climate_gdd: vec![1000; 4],
                climate_rainfall_mm: vec![1000; 4],
                glacier: vec![false; 4],
                hillside: vec![0; 4],
                ice: ck_map::Rgba8::opaque(232, 233, 237),
                ice_second: ck_map::Rgba8::opaque(185, 188, 198),
                terrains: Vec::new(),
                sea: water,
                shallow_sea: water,
                lake: water,
                river: water,
                shallow_threshold: 1,
            },
            terrain_types: vec![ck_map::LoadedTerrain {
                key: [0, 0, 0],
                kind: TerrainKind::Sea,
                palette_colour: water,
                style_index: None,
            }],
            grid_a: ck_map::PackedGrid {
                width: crop.width,
                height: crop.height,
                pixels: vec![[0, 0, 128, 255]; 4],
            },
            grid_b: ck_map::PackedGrid {
                width: crop.width,
                height: crop.height,
                pixels: vec![[0x2a, 0xf8, 0, 255]; 4],
            },
            rivers: Vec::new(),
        }
    }

    fn loaded_map_with_rivers(rivers: Vec<ck_map::RiverLine>) -> LoadedMap {
        let mut loaded = loaded_map();
        loaded.rivers = rivers;
        loaded
    }

    fn expanded_map() -> LoadedMap {
        let mut loaded = loaded_map();
        let crop = Crop {
            width: 200,
            height: 160,
            ..loaded.data.crop
        };
        let length = usize::try_from(crop.width * crop.height).expect("fixture dimensions fit");
        loaded.data.crop = crop;
        loaded.data.cells = vec![ck_map::Surface::Sea { depth: 0 }; length];
        loaded.data.elevations = vec![11_000; length];
        loaded.grid_a.width = crop.width;
        loaded.grid_a.height = crop.height;
        loaded.grid_a.pixels = vec![[0, 0, 128, 255]; length];
        loaded.grid_b.width = crop.width;
        loaded.grid_b.height = crop.height;
        loaded.grid_b.pixels = vec![[0x2a, 0xf8, 0, 255]; length];
        loaded
    }

    fn controller(pan: u32, zoom: u32) -> MapController {
        controller_with_controls(pan, zoom, 24)
    }

    fn controller_with_controls(pan: u32, zoom: u32, scroll: u32) -> MapController {
        let loaded = loaded_map();
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut controller = MapController::new(&loaded, bridge, pan, zoom, scroll)
            .expect("map lane accepts fixture");
        controller
            .open(&WindowDesc {
                title: "map controller test".to_owned(),
                width: 400,
                height: 320,
                clear: [0.0; 3],
                ..WindowDesc::default()
            })
            .expect("initial view is valid");
        controller
    }

    fn expanded_controller() -> MapController {
        expanded_controller_with(400, 320, 24)
    }

    fn expanded_controller_with(
        width: u32,
        height: u32,
        scroll_points_per_step: u32,
    ) -> MapController {
        let loaded = expanded_map();
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut controller = MapController::new(&loaded, bridge, 4, 20, scroll_points_per_step)
            .expect("map lane accepts expanded fixture");
        controller
            .open(&WindowDesc {
                title: "pointer controller test".to_owned(),
                width,
                height,
                clear: [0.0; 3],
                ..WindowDesc::default()
            })
            .expect("initial view is valid");
        controller
    }

    pub(crate) fn hover_controller() -> MapController {
        hover_controller_with_bridge(SceneBridge::new(ck_render_2d::Filter::Nearest))
    }

    pub(crate) fn hover_controller_with_bridge(bridge: Arc<SceneBridge>) -> MapController {
        hover_controller_with_glyph_and_bridge(vec![vec!['#', 'd'], vec!['d', '#']], bridge)
    }

    fn hover_controller_with_glyph(glyph: Vec<Vec<char>>) -> MapController {
        hover_controller_with_glyph_and_bridge(
            glyph,
            SceneBridge::new(ck_render_2d::Filter::Nearest),
        )
    }

    fn hover_controller_with_glyph_and_bridge(
        glyph: Vec<Vec<char>>,
        bridge: Arc<SceneBridge>,
    ) -> MapController {
        let loaded = expanded_map();
        let crop = loaded.data.crop;
        let cell = ck_map::Cell {
            x: crop.origin.x + 100,
            y: crop.origin.y + 80,
        };
        let kind = ck_map::LocationKind {
            id: "test".to_owned(),
            min_size: 1,
            max_size: 1,
            catchment_m: vec![0],
            house_count: vec![0],
            house_radius_cells_x256: vec![0],
            glyph,
            light: ck_map::Rgba8::opaque(255, 0, 0),
            dark: ck_map::Rgba8::opaque(0, 0, 0),
        };
        let location = Location {
            id: "fixture-town".to_owned(),
            name: "Fixture Town".to_owned(),
            cell,
            kind_index: 0,
            size: 1,
            building_indices: Vec::new(),
            catchment_m: None,
            source: "test".to_owned(),
            confidence: ck_map::Confidence::High,
        };
        let kinds = LocationKinds {
            kinds: vec![kind],
            ..LocationKinds::default()
        };
        let mut controller = MapController::with_scenario_parts(
            &loaded,
            bridge,
            MapControls {
                pan_pixels_per_frame: 1,
                zoom_steps_per_second: 20,
                scroll_points_per_step: 24,
            },
            PreparedRivers::new(Vec::new()),
            vec![location],
            kinds,
            &[],
        )
        .expect("map lane accepts hover fixture");
        let desc = WindowDesc {
            title: "hover test".to_owned(),
            width: 400,
            height: 320,
            clear: [0.0; 3],
            ..WindowDesc::default()
        };
        controller.open(&desc).expect("initial view is valid");
        let center = FixedPoint {
            x: i32::try_from(
                i64::from(cell.x) * ck_map::FIXED_PER_CELL + ck_map::FIXED_PER_CELL / 2,
            )
            .expect("fixture center fits"),
            y: i32::try_from(
                i64::from(cell.y) * ck_map::FIXED_PER_CELL + ck_map::FIXED_PER_CELL / 2,
            )
            .expect("fixture center fits"),
        };
        controller.camera = Some(
            Camera::new(46, center, ViewSize::from_points(400, 320), crop)
                .expect("fixture marker fits camera"),
        );
        controller
    }

    /// Mirrors the CK-198 window fixture: its 2 x 2 map, 400 x 240 view, one place at the
    /// southeast cell, and the same one-point-per-zoom-step scroll control.
    pub(crate) fn window_case_hover_controller() -> MapController {
        let loaded = loaded_map();
        let crop = loaded.data.crop;
        let kind = ck_map::LocationKind {
            id: "town".to_owned(),
            min_size: 1,
            max_size: 1,
            catchment_m: vec![839],
            house_count: vec![0],
            house_radius_cells_x256: vec![0],
            glyph: vec![vec!['#']],
            light: ck_map::Rgba8::opaque(255, 255, 255),
            dark: ck_map::Rgba8::opaque(0, 0, 0),
        };
        let location = Location {
            id: "fixture_town".to_owned(),
            name: "Fixture Town".to_owned(),
            cell: ck_map::Cell {
                x: crop.origin.x + 1,
                y: crop.origin.y + 1,
            },
            kind_index: 0,
            size: 1,
            building_indices: Vec::new(),
            catchment_m: None,
            source: "fixture".to_owned(),
            confidence: ck_map::Confidence::High,
        };
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut controller = MapController::with_scenario_parts(
            &loaded,
            bridge,
            MapControls {
                pan_pixels_per_frame: 1,
                zoom_steps_per_second: 20,
                scroll_points_per_step: 1,
            },
            PreparedRivers::new(Vec::new()),
            vec![location],
            LocationKinds {
                kinds: vec![kind],
                ..LocationKinds::default()
            },
            &[],
        )
        .expect("map lane accepts window-case hover fixture");
        controller
            .open(&WindowDesc {
                title: "window-case hover fixture".to_owned(),
                width: 400,
                height: 240,
                clear: [0.0; 3],
                ..WindowDesc::default()
            })
            .expect("window-case initial view is valid");
        controller
    }

    pub(crate) fn hover_pointer(controller: &mut MapController) -> PointerPosition {
        let camera = controller.camera.expect("opened camera");
        let bounds = controller
            .markers
            .hit_bounds(camera.step, 0)
            .expect("fixture marker bounds");
        let top_left = camera.top_left();
        let x = (i64::from(bounds.0.x) - i64::from(top_left.x)) * 4;
        let y = (i64::from(bounds.0.y) - i64::from(top_left.y)) * 4;
        PointerPosition {
            x: i32::try_from(x * 256).expect("pointer x fits"),
            y: i32::try_from(y * 256).expect("pointer y fits"),
        }
    }

    pub(crate) fn select_fixture(controller: &mut MapController) {
        controller
            .camera
            .as_mut()
            .expect("opened camera")
            .set_step(46);
        let pointer = hover_pointer(controller);
        controller
            .update(&pointer_update(
                vec![PointerEvent::Down(pointer), PointerEvent::Up(pointer)],
                Some(pointer),
                false,
                false,
            ))
            .expect("fixture marker click is valid");
    }

    fn update(actions: &[(&str, bool, bool)], dt: Duration) -> InputTick {
        InputTick {
            dt,
            metrics: WindowMetrics {
                points: (400, 320),
                pixels: PixelSize {
                    width: 800,
                    height: 640,
                },
            },
            input: InputSnapshot {
                sequence: 1,
                actions: actions
                    .iter()
                    .map(|(name, held, pressed)| ActionSnapshot {
                        name: (*name).to_owned(),
                        held: *held,
                        pressed: *pressed,
                        released: false,
                    })
                    .collect(),
                transitions: Vec::new(),
                overflow: false,
                reset: false,
                overflow_count: 0,
                pointer_events: Vec::new(),
                pointer_position: None,
                pointer_held: false,
                pointer_overflow: false,
            },
        }
    }

    fn pointer_update(
        events: Vec<PointerEvent>,
        position: Option<PointerPosition>,
        held: bool,
        overflow: bool,
    ) -> InputTick {
        let mut update = update(&[], Duration::ZERO);
        update.input.pointer_events = events;
        update.input.pointer_position = position;
        update.input.pointer_held = held;
        update.input.pointer_overflow = overflow;
        update
    }

    fn pointer_update_in_view(
        width: u32,
        height: u32,
        events: Vec<PointerEvent>,
        position: Option<PointerPosition>,
        held: bool,
        overflow: bool,
    ) -> InputTick {
        let mut update = pointer_update(events, position, held, overflow);
        update.metrics.points = (width, height);
        update.metrics.pixels = PixelSize {
            width: width.saturating_mul(2),
            height: height.saturating_mul(2),
        };
        update
    }

    fn world_pixel_under(camera: Camera, position: PointerPosition) -> (i64, i64) {
        let pixel = i64::from(STEP_SIZE[usize::from(camera.step)]);
        let cursor_x = i64::from(position.x).div_euclid(4 * 256);
        let cursor_y = i64::from(position.y).div_euclid(4 * 256);
        let top_left = camera.top_left();
        (
            (i64::from(top_left.x) + cursor_x) * pixel + pixel / 2,
            (i64::from(top_left.y) + cursor_y) * pixel + pixel / 2,
        )
    }

    fn at_step<R: RiverViewSelector>(controller: &mut MapController<R>, step: u8) -> Camera {
        let current = controller.camera.expect("opened camera");
        let crop = controller.crop;
        let center = FixedPoint {
            x: i32::try_from(
                i64::from(crop.origin.x) * ck_map::FIXED_PER_CELL
                    + i64::from(crop.width) * ck_map::FIXED_PER_CELL / 2,
            )
            .expect("fixture center fits"),
            y: i32::try_from(
                i64::from(crop.origin.y) * ck_map::FIXED_PER_CELL
                    + i64::from(crop.height) * ck_map::FIXED_PER_CELL / 2,
            )
            .expect("fixture center fits"),
        };
        let camera = Camera::new(step, center, current.view, crop).expect("fixture camera");
        controller.camera = Some(camera);
        camera
    }

    fn river_test_controller() -> (
        MapController<CountingRivers>,
        Arc<SceneBridge>,
        Arc<CapturingSink>,
    ) {
        let center = FixedPoint {
            x: i32::try_from(23_883 * ck_map::FIXED_PER_CELL).expect("fixture coordinate fits"),
            y: i32::try_from(23_883 * ck_map::FIXED_PER_CELL).expect("fixture coordinate fits"),
        };
        let loaded = loaded_map_with_rivers(vec![ck_map::RiverLine {
            points: vec![center],
        }]);
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut controller = MapController::with_rivers(
            &loaded,
            Arc::clone(&bridge),
            1,
            20,
            24,
            CountingRivers::new(loaded.rivers.clone()),
        )
        .expect("map lane accepts fixture");
        let desc = WindowDesc {
            title: "river controller test".to_owned(),
            width: 400,
            height: 320,
            clear: [0.0; 3],
            ..WindowDesc::default()
        };
        controller.open(&desc).expect("initial view is valid");
        let sink = Arc::new(CapturingSink::default());
        bridge
            .attach(
                &desc,
                PixelSize {
                    width: 800,
                    height: 640,
                },
                sink.clone(),
            )
            .expect("test sink attaches");
        (controller, bridge, sink)
    }

    fn expected_center_river(camera: Camera) -> [u32; 2] {
        let top_left = camera.top_left();
        let pixel_size = i64::from(STEP_SIZE[usize::from(camera.step)]);
        let world = WorldPixel {
            x: i32::try_from((23_883 * ck_map::FIXED_PER_CELL) / pixel_size).expect("world x fits"),
            y: i32::try_from((23_883 * ck_map::FIXED_PER_CELL) / pixel_size).expect("world y fits"),
        };
        [
            u32::try_from(i64::from(world.x) - i64::from(top_left.x)).expect("river x in view"),
            u32::try_from(i64::from(world.y) - i64::from(top_left.y)).expect("river y in view"),
        ]
    }

    #[test]
    fn the_rule_table_carries_each_terrains_ten_tones_and_the_ice_of_the_shipped_map() {
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/map/ireland");
        let loaded = ck_map::load_map(&directory).expect("Ireland's map loads");
        let rules = rules(&loaded, &[]);
        assert_eq!(rules.terrains.len(), loaded.terrain_types.len());
        for (rule, terrain) in rules.terrains.iter().zip(&loaded.terrain_types) {
            if let Some(index) = terrain.style_index {
                let style = loaded.data.terrains[usize::from(index)];
                // The ends of the ten tones are the data's four colours: the dark and full colours, the second tone at the top of the
                // shadow side and the lit-dark tone at the bottom of the lit side.
                assert_eq!(
                    (rule.tone[0], rule.tone[3], rule.shadow[2], rule.lit[0]),
                    (
                        rgba(style.dark),
                        rgba(style.solid),
                        rgba(style.second),
                        rgba(style.dark_lit)
                    )
                );
                let shades = TerrainShades::of(&style);
                assert_eq!(rule.tone, shades.tone.map(rgba));
                assert_eq!(rule.shadow, shades.shadow.map(rgba));
                assert_eq!(rule.lit, shades.lit.map(rgba));
            }
        }
        // The shipped data: the ice colour is the old snow cap's, with its own second tone; the sea and rivers are as before.
        assert_eq!(rules.ice, [0xe8, 0xe9, 0xed, 255]);
        assert_ne!(rules.ice_second, rules.ice);
        assert_eq!(rules.river, rgba(loaded.data.river));
        // The lane accepts the shipped map's grid A: fertility ÷ 4 in G (255 for a glacier) and the strength + 128 in B.
        let input = MapInput::new(
            MAP_INPUT_GENERATION,
            [loaded.data.crop.origin.x, loaded.data.crop.origin.y],
            [loaded.data.crop.width, loaded.data.crop.height],
            flatten(&loaded.grid_a.pixels),
            flatten(&loaded.grid_b.pixels),
            rules,
        );
        assert!(input.is_ok(), "{:?}", input.err());
        assert!(
            loaded.grid_a.pixels.iter().any(|pixel| pixel[2] != 128),
            "the shipped map has hillsides"
        );
        assert!(
            loaded
                .grid_a
                .pixels
                .iter()
                .any(|pixel| pixel[1] > 0 && pixel[1] <= 250),
            "and fertility"
        );
    }

    #[test]
    fn initial_step_is_the_coarsest_one_that_fits_the_whole_crop() {
        let crop = Crop {
            origin: ck_map::Cell { x: 100, y: 100 },
            width: 60,
            height: 40,
        };
        let view = ViewSize {
            width: 20,
            height: 20,
        };
        let step = initial_step(crop, view);
        assert!(
            i64::from(crop.width) * ck_map::FIXED_PER_CELL
                <= i64::from(view.width) * i64::from(STEP_SIZE[usize::from(step)])
        );
        if step > 0 {
            let coarser = step - 1;
            assert!(
                i64::from(crop.width) * ck_map::FIXED_PER_CELL
                    > i64::from(view.width) * i64::from(STEP_SIZE[usize::from(coarser)])
                    || i64::from(crop.height) * ck_map::FIXED_PER_CELL
                        > i64::from(view.height) * i64::from(STEP_SIZE[usize::from(coarser)])
            );
        }
    }

    #[test]
    fn initial_view_uses_step_zero_and_centres_a_crop_that_cannot_fit() {
        let crop = Crop {
            origin: ck_map::Cell { x: 100, y: 200 },
            width: 2_000,
            height: 2_000,
        };
        let view = ViewSize {
            width: 20,
            height: 20,
        };

        let camera = initial_camera(crop, view).expect("initial camera is valid");

        assert_eq!(camera.step, 0);
        assert_eq!(
            camera.center,
            FixedPoint {
                x: i32::try_from(1_100 * ck_map::FIXED_PER_CELL).unwrap(),
                y: i32::try_from(1_200 * ck_map::FIXED_PER_CELL).unwrap(),
            }
        );
    }

    #[test]
    fn zoom_step_clamps_at_both_limits() {
        let crop = Crop {
            origin: ck_map::Cell { x: 10, y: 10 },
            width: 2,
            height: 2,
        };
        let view = ViewSize {
            width: 8,
            height: 8,
        };
        let mut camera = Camera::new(
            0,
            FixedPoint {
                x: 10 * 32768,
                y: 10 * 32768,
            },
            view,
            crop,
        )
        .unwrap();
        assert!(!change_step(&mut camera, -1));
        camera.set_step(75);
        assert!(!change_step(&mut camera, 1));
    }

    #[test]
    fn initial_camera_is_coarse_and_centred_on_the_loaded_crop() {
        let controller = controller(4, 20);
        let camera = controller.camera().expect("initial camera");
        assert_eq!(camera.center.x, 23883 * 32_768);
        assert_eq!(camera.center.y, 23883 * 32_768);
        assert_eq!(camera.step, initial_step(controller.crop, camera.view));
    }

    #[test]
    fn river_selection_reuses_step_coverage_and_publishes_visible_pixels() {
        let (mut controller, _bridge, sink) = river_test_controller();
        assert_eq!(
            controller.rivers.preparations, 1,
            "opening prepares its step"
        );

        let initial = controller.camera().expect("opened camera");
        let initial_frame = sink.latest();
        let initial_map = initial_frame.map.expect("map frame is published");
        assert_eq!(
            initial_map.camera.rivers().as_ref(),
            [expected_center_river(initial)],
            "the camera snapshot carries the expected river pixel in view coordinates"
        );

        assert!(
            controller
                .update(&update(
                    &[(PAN_RIGHT, true, true)],
                    Duration::from_millis(16)
                ))
                .expect("pan publishes")
        );
        assert_eq!(controller.rivers.preparations, 1, "panning reuses coverage");

        at_step(&mut controller, 10);
        assert!(
            controller
                .update(&update(&[(ZOOM_IN, true, true)], Duration::from_millis(16)))
                .expect("step change publishes")
        );
        assert_eq!(
            controller.rivers.preparations, 2,
            "the new step prepares once"
        );

        let second_read = controller.camera().expect("camera remains open");
        controller
            .publish_for_generation(second_read, controller.bridge.metrics_generation())
            .expect("second read at the new step publishes");
        assert_eq!(
            controller.rivers.preparations, 2,
            "a second read at the new step reuses the prepared coverage"
        );
        let current = controller.camera().expect("camera remains open");
        let published = sink.latest().map.expect("map frame is published");
        assert_eq!(
            published.camera.rivers().as_ref(),
            [expected_center_river(current)]
        );
    }

    #[test]
    fn each_pan_direction_moves_the_configured_map_pixels() {
        for (name, dx, dy) in [
            (PAN_LEFT, -1, 0),
            (PAN_RIGHT, 1, 0),
            (PAN_UP, 0, -1),
            (PAN_DOWN, 0, 1),
        ] {
            let mut controller = controller(3, 20);
            let before = at_step(&mut controller, 75);
            assert!(
                controller
                    .update(&update(&[(name, true, true)], Duration::from_millis(16)))
                    .expect("pan publishes")
            );
            let after = controller.camera().expect("camera remains open");
            let pixels = i64::from(STEP_SIZE[75]) * 3;
            assert_eq!(
                i64::from(after.center.x) - i64::from(before.center.x),
                i64::from(dx) * pixels
            );
            assert_eq!(
                i64::from(after.center.y) - i64::from(before.center.y),
                i64::from(dy) * pixels
            );
        }
    }

    #[test]
    fn changed_controls_take_effect_on_the_next_input_tick() {
        let mut pan = controller(1, 20);
        let before = at_step(&mut pan, 75);
        pan.set_controls(5, 20, 24);
        pan.update(&update(&[(PAN_RIGHT, true, false)], Duration::ZERO))
            .expect("updated pan publishes");
        let after = pan.camera().expect("camera remains open");
        assert_eq!(
            i64::from(after.center.x) - i64::from(before.center.x),
            i64::from(STEP_SIZE[75]) * 5
        );

        let mut zoom = controller(4, 20);
        at_step(&mut zoom, 10);
        zoom.set_controls(4, 1, 24);
        zoom.update(&update(&[(ZOOM_IN, true, false)], Duration::from_secs(1)))
            .expect("updated zoom publishes");
        assert_eq!(zoom.camera().expect("camera remains open").step, 11);

        let mut scroll = expanded_controller();
        let before = scroll.camera().expect("camera remains open").step;
        scroll.set_controls(4, 20, 1);
        scroll
            .update(&pointer_update(
                vec![PointerEvent::Scroll {
                    position: PointerPosition {
                        x: 160 * 256,
                        y: 120 * 256,
                    },
                    amount: 256,
                    unit: ScrollUnit::Points,
                }],
                Some(PointerPosition {
                    x: 160 * 256,
                    y: 120 * 256,
                }),
                false,
                false,
            ))
            .expect("updated scroll publishes");
        assert_eq!(
            scroll.camera().expect("camera remains open").step,
            before + 1
        );
    }

    #[test]
    fn settings_set_and_reset_change_all_map_controls_on_the_next_tick() {
        let dir = crate::testing::Dir::new();
        let config_path = dir.0.join("config.toml");
        std::fs::write(
            &config_path,
            "[map]\ndirectory = 'fixture'\npan_pixels_per_frame = 2\nzoom_steps_per_second = 3\nscroll_points_per_step = 2\n",
        )
        .expect("write config");
        let config = crate::config::load(&config_path).expect("load config and settings");
        let mut map = controller_with_controls(
            config.map_settings.pan_pixels_per_frame,
            config.map_settings.zoom_steps_per_second,
            config.map_settings.scroll_points_per_step,
        );
        at_step(&mut map, 30);
        let shared = shared(map);
        let runtime = crate::settings::Runtime::new(&config, Some(Arc::clone(&shared)))
            .expect("settings runtime with a map");
        let map_for_tick = Arc::clone(&shared);
        let map_tick = Extension::new("map_test").function(
            "tick",
            Arc::new(move |args| {
                let Some(Some(ConfigValue::String(control))) = args.first() else {
                    return Err("map_test.tick: expected a control name".to_owned());
                };
                let mut map = lock(&map_for_tick);
                match control.as_str() {
                    "pan" => {
                        at_step(&mut map, 75);
                    }
                    "zoom" => {
                        at_step(&mut map, 10);
                    }
                    "scroll" => {
                        at_step(&mut map, 30);
                    }
                    _ => return Err(format!("map_test.tick: unknown control `{control}`")),
                }
                let before = map.camera().ok_or("map_test.tick: no camera")?;
                let input = match control.as_str() {
                    "pan" => update(&[(PAN_RIGHT, true, false)], Duration::ZERO),
                    "zoom" => update(&[(ZOOM_IN, true, false)], Duration::from_secs(1)),
                    "scroll" => pointer_update(
                        vec![PointerEvent::Scroll {
                            position: PointerPosition {
                                x: 160 * 256,
                                y: 120 * 256,
                            },
                            amount: 512,
                            unit: ScrollUnit::Points,
                        }],
                        Some(PointerPosition {
                            x: 160 * 256,
                            y: 120 * 256,
                        }),
                        false,
                        false,
                    ),
                    _ => unreachable!("control was validated above"),
                };
                map.update(&input)?;
                let after = map
                    .camera()
                    .ok_or("map_test.tick: no camera after update")?;
                let amount = if control == "pan" {
                    (i64::from(after.center.x) - i64::from(before.center.x))
                        / i64::from(STEP_SIZE[usize::from(before.step)])
                } else {
                    i64::from(after.step) - i64::from(before.step)
                };
                Ok(vec![ConfigValue::Integer(amount)])
            }),
        );
        let script = r#"
settings.set("map.pan_pixels_per_frame", 7)
local pan_set = map_test.tick("pan")
assert(pan_set == 7, "set pan used " .. tostring(pan_set))
settings.set("map.zoom_steps_per_second", 5)
local zoom_set = map_test.tick("zoom")
assert(zoom_set == 5, "set zoom used " .. tostring(zoom_set))
settings.set("map.scroll_points_per_step", 1)
local scroll_set = map_test.tick("scroll")
assert(scroll_set == 2, "set scroll used " .. tostring(scroll_set))
settings.reset("map")
local pan_reset = map_test.tick("pan")
assert(pan_reset == 2, "reset pan used " .. tostring(pan_reset))
local zoom_reset = map_test.tick("zoom")
assert(zoom_reset == 3, "reset zoom used " .. tostring(zoom_reset))
local scroll_reset = map_test.tick("scroll")
assert(scroll_reset == 1, "reset scroll used " .. tostring(scroll_reset))
"#;
        let _harness = crate::testing::Harness::new(
            script,
            crate::testing::Extras {
                extensions: vec![runtime.extension(), map_tick],
                ..Default::default()
            },
        );
    }

    #[test]
    fn zoom_press_and_repeat_keep_the_exact_centre() {
        let mut controller = controller(4, 20);
        let before = at_step(&mut controller, 10);
        assert!(
            controller
                .update(&update(&[(ZOOM_IN, true, true)], Duration::from_millis(16)))
                .expect("zoom press publishes")
        );
        let pressed = controller.camera().expect("camera remains open");
        assert_eq!(pressed.step, 11);
        assert_eq!(pressed.center, before.center);
        assert!(
            controller
                .update(&update(
                    &[(ZOOM_IN, true, false)],
                    Duration::from_millis(100)
                ))
                .expect("held zoom publishes")
        );
        let repeated = controller.camera().expect("camera remains open");
        assert_eq!(
            repeated.step, 13,
            "20 steps per second repeats twice in 100 ms"
        );
        assert_eq!(repeated.center, before.center);
    }

    #[test]
    fn zoom_limits_and_crop_edges_are_clamped() {
        let mut controller = controller(32, 20);
        at_step(&mut controller, 0);
        assert!(
            !controller
                .update(&update(&[(ZOOM_OUT, true, true)], Duration::ZERO))
                .expect("clamped zoom is harmless")
        );
        at_step(&mut controller, 75);
        assert!(
            !controller
                .update(&update(&[(ZOOM_IN, true, true)], Duration::ZERO))
                .expect("clamped zoom is harmless")
        );

        let crop = controller.crop;
        controller
            .update(&update(&[(PAN_LEFT, true, true)], Duration::ZERO))
            .expect("pan is valid");
        let edge = controller.camera().expect("camera remains open");
        assert_eq!(edge.center.x, crop.origin.x * 32_768);
        controller
            .update(&update(&[(PAN_LEFT, true, false)], Duration::ZERO))
            .expect("repeated edge pan is valid");
        assert_eq!(
            controller.camera().expect("camera remains open").center.x,
            edge.center.x
        );
    }

    #[test]
    fn drag_replays_a_complete_gesture_and_applies_exact_whole_pixel_deltas() {
        let mut controller = expanded_controller();
        let before = controller.camera().expect("initial camera");
        let anchor = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let end = PointerPosition {
            x: 46 * 256,
            y: 31 * 256,
        };
        assert!(
            !controller
                .update(&pointer_update(
                    vec![
                        PointerEvent::ButtonDown {
                            button: ck_window::PointerButton::Middle,
                            position: anchor,
                        },
                        PointerEvent::ButtonUp {
                            button: ck_window::PointerButton::Right,
                            position: end,
                        },
                    ],
                    Some(end),
                    false,
                    false,
                ))
                .expect("non-left buttons are ignored by the map"),
            "non-left button edges do not begin, move, end or select a map gesture"
        );
        assert_eq!(controller.camera(), Some(before));
        assert_eq!(controller.selection(), None);
        assert!(
            controller
                .update(&pointer_update(
                    vec![
                        PointerEvent::Down(anchor),
                        PointerEvent::Move(end),
                        PointerEvent::Up(end)
                    ],
                    Some(end),
                    false,
                    false,
                ))
                .expect("complete drag publishes")
        );
        let after = controller.camera().expect("camera remains open");
        let pixel = i64::from(STEP_SIZE[usize::from(before.step)]);
        assert_eq!(
            i64::from(after.center.x) - i64::from(before.center.x),
            -pixel
        );
        assert_eq!(
            i64::from(after.center.y) - i64::from(before.center.y),
            3 * pixel
        );
        assert!(
            !controller
                .update(&pointer_update(Vec::new(), Some(end), false, false))
                .expect("consumed pointer is idle")
        );
    }

    #[test]
    fn only_a_retained_map_click_selects_a_marker_and_each_selection_advances_seq() {
        let mut controller = hover_controller();
        let marker = hover_pointer(&mut controller);
        for seq in [1, 2] {
            controller
                .update(&pointer_update(
                    vec![PointerEvent::Down(marker), PointerEvent::Up(marker)],
                    Some(marker),
                    false,
                    false,
                ))
                .expect("marker click is accepted");
            assert_eq!(
                controller.selection(),
                Some(Selection {
                    seq,
                    id: "fixture-town".to_owned(),
                    name: "Fixture Town".to_owned(),
                    kind: "test".to_owned(),
                    size: 1,
                })
            );
        }
    }

    #[test]
    fn a_map_gesture_that_crosses_a_pixel_is_never_a_click_even_when_it_returns() {
        let mut controller = hover_controller();
        let marker = hover_pointer(&mut controller);
        let moved = PointerPosition {
            x: marker.x + 4 * 256,
            y: marker.y,
        };
        controller
            .update(&pointer_update(
                vec![
                    PointerEvent::Down(marker),
                    PointerEvent::Move(moved),
                    PointerEvent::Move(marker),
                    PointerEvent::Up(marker),
                ],
                Some(marker),
                false,
                false,
            ))
            .expect("out and back gesture is valid");
        assert_eq!(controller.selection(), None);
    }

    #[test]
    fn empty_reset_replaced_up_and_crop_clamped_gestures_do_not_select() {
        assert_empty_map_click_does_not_select();

        let mut reset = hover_controller();
        let marker = hover_pointer(&mut reset);
        reset
            .update(&pointer_update(
                vec![PointerEvent::Down(marker)],
                Some(marker),
                true,
                false,
            ))
            .expect("map gesture starts");
        // CK-207 routes a reset as an ownership-aware EndDrag before replaying any later input.
        reset.end_drag();
        reset
            .update(&pointer_update(
                vec![PointerEvent::Up(marker)],
                Some(marker),
                false,
                false,
            ))
            .expect("late up after reset is harmless");
        assert_eq!(reset.selection(), None, "reset makes the click ineligible");

        let mut replaced = hover_controller();
        let marker = hover_pointer(&mut replaced);
        replaced
            .update(&pointer_update(
                vec![PointerEvent::Down(marker)],
                Some(marker),
                true,
                false,
            ))
            .expect("old gesture starts");
        replaced.end_drag(); // CK-207's lost-Up ownership end before the replacement Down.
        replaced
            .update(&pointer_update(
                vec![PointerEvent::Down(marker)],
                Some(marker),
                true,
                false,
            ))
            .expect("replacement Down starts a distinct gesture");
        assert_eq!(
            replaced.selection(),
            None,
            "a Down cannot complete a lost-Up gesture"
        );

        let mut clamped = hover_controller();
        assert_crop_clamped_drag_does_not_select(&mut clamped);
    }

    fn assert_crop_clamped_drag_does_not_select(clamped: &mut MapController) {
        let initial = clamped.camera.expect("camera is open");
        let crop_right = i32::try_from(
            (i64::from(clamped.crop.origin.x) + i64::from(clamped.crop.width))
                * ck_map::FIXED_PER_CELL,
        )
        .expect("crop edge fits");
        let pan_pixel = STEP_SIZE[46];
        clamped.camera = Some(
            Camera::new(
                46,
                FixedPoint {
                    // Start inside the crop. The drag below requests a full pixel eastward,
                    // crossing this short remainder so Camera::pan must clamp at the edge.
                    x: crop_right - pan_pixel / 2,
                    y: initial.center.y,
                },
                initial.view,
                clamped.crop,
            )
            .expect("camera clamps to crop edge"),
        );
        let before = clamped.camera.expect("camera before crop-clamped drag");
        assert!(
            before.center.x < crop_right,
            "the camera starts inside the crop"
        );
        let attempted_center = i64::from(before.center.x) + i64::from(pan_pixel);
        assert!(
            attempted_center > i64::from(crop_right),
            "the pan crosses the crop edge"
        );
        let mut edge_camera = before;
        edge_camera.pan(1, 0, clamped.crop);
        assert_eq!(
            edge_camera.center.x, crop_right,
            "the requested pan clamps at the edge"
        );
        clamped.camera = Some(edge_camera);
        let moved = hover_pointer(clamped);
        clamped.camera = Some(before);
        // Start one point right of the endpoint: the input quantizes to a one-pixel
        // eastward pan, while the endpoint is the marker's position at the clamped edge.
        let anchor = PointerPosition {
            x: moved.x + 256,
            y: moved.y,
        };
        let click_mutation_selection = clamped.selected_at(edge_camera, moved);
        assert_eq!(
            click_mutation_selection
                .as_ref()
                .map(|selection| selection.id.as_str()),
            Some("fixture-town"),
            "the endpoint remains on the marker, so a gesture-as-click mutation is observable"
        );
        clamped
            .update(&pointer_update(
                vec![
                    PointerEvent::Down(anchor),
                    PointerEvent::Move(moved),
                    PointerEvent::Up(moved),
                ],
                Some(moved),
                false,
                false,
            ))
            .expect("crop-clamped drag is valid");
        let after = clamped.camera.expect("camera after crop-clamped drag");
        assert_eq!(
            after.center.x, crop_right,
            "the pan is clamped at the crop edge"
        );
        assert_eq!(
            clamped.selection(),
            None,
            "crossing a pixel latches a drag even when clamped"
        );
    }

    fn assert_empty_map_click_does_not_select() {
        let mut empty = hover_controller();
        let outside_marker = PointerPosition { x: 0, y: 0 };
        empty
            .update(&pointer_update(
                vec![
                    PointerEvent::Down(outside_marker),
                    PointerEvent::Up(outside_marker),
                ],
                Some(outside_marker),
                false,
                false,
            ))
            .expect("empty-map click is valid");
        assert_eq!(empty.selection(), None, "empty map has no place to select");
    }

    #[test]
    fn raw_overflow_disqualifies_the_whole_map_gesture_including_an_up_tick() {
        let mut controller = hover_controller();
        let marker = hover_pointer(&mut controller);
        controller
            .update(&pointer_update(
                vec![PointerEvent::Down(marker)],
                Some(marker),
                true,
                false,
            ))
            .expect("map gesture starts");
        controller
            .update(&pointer_update(
                vec![PointerEvent::Up(marker)],
                Some(marker),
                false,
                true,
            ))
            .expect("overflow tick ends gesture without selecting");
        assert_eq!(controller.selection(), None);

        controller
            .update(&pointer_update(
                vec![PointerEvent::Down(marker), PointerEvent::Up(marker)],
                Some(marker),
                false,
                true,
            ))
            .expect("same tick loss is conservative");
        assert_eq!(controller.selection(), None);
    }

    #[test]
    fn a_marker_is_not_selectable_before_the_prepared_hit_step() {
        let mut controller = hover_controller();
        let marker_at_step_46 = hover_pointer(&mut controller);
        let initial = controller.camera.expect("opened camera");
        controller.camera = Some(
            Camera::new(14, initial.center, initial.view, controller.crop)
                .expect("step 14 fixture camera is valid"),
        );
        controller
            .update(&pointer_update(
                vec![
                    PointerEvent::Down(marker_at_step_46),
                    PointerEvent::Up(marker_at_step_46),
                ],
                Some(marker_at_step_46),
                false,
                false,
            ))
            .expect("pre-marker click is valid");
        assert_eq!(controller.selection(), None);
    }

    #[test]
    fn an_ownership_end_drops_the_old_drag_without_panning_on_later_ticks() {
        let mut controller = expanded_controller();
        let before = controller.camera().expect("initial camera");
        let anchor = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let moved = PointerPosition {
            x: 48 * 256,
            y: 40 * 256,
        };
        let mut active = pointer_update(vec![], Some(moved), true, false);
        active.input.pointer_events.clear();
        controller
            .update_routed(
                &active,
                &[
                    RoutedMapEvent::Pointer(PointerEvent::Down(anchor)),
                    RoutedMapEvent::Pointer(PointerEvent::Move(moved)),
                ],
                None,
                true,
                false,
            )
            .expect("map gesture pans before ownership changes");
        let after_map_gesture = controller.camera().expect("camera after map drag");
        assert_ne!(after_map_gesture, before);

        let ended = pointer_update(Vec::new(), None, false, false);
        controller
            .update_routed(&ended, &[RoutedMapEvent::EndDrag], None, true, false)
            .expect("ending ownership does not pan");
        let later = pointer_update(Vec::new(), Some(moved), true, false);
        controller
            .update_routed(&later, &[], None, true, false)
            .expect("later UI-owned movement does not reach map");
        assert_eq!(controller.camera(), Some(after_map_gesture));
    }

    #[test]
    fn drag_keeps_fractional_remainders_and_returns_exactly_to_its_anchor() {
        let mut controller = expanded_controller();
        let before = at_step(&mut controller, 74);
        let anchor = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        controller
            .update(&pointer_update(
                vec![PointerEvent::Down(anchor)],
                Some(anchor),
                true,
                false,
            ))
            .expect("drag starts at its anchor");
        for x in [41, 42, 43, 44] {
            controller
                .update(&pointer_update(
                    vec![PointerEvent::Move(PointerPosition {
                        x: x * 256,
                        y: anchor.y,
                    })],
                    Some(PointerPosition {
                        x: x * 256,
                        y: anchor.y,
                    }),
                    true,
                    false,
                ))
                .expect("small drag move succeeds");
        }
        let moved = controller.camera().expect("moved camera");
        let pixel = i64::from(STEP_SIZE[usize::from(before.step)]);
        assert_eq!(
            i64::from(moved.center.x) - i64::from(before.center.x),
            -pixel
        );
        controller
            .update(&pointer_update(
                vec![PointerEvent::Move(anchor), PointerEvent::Up(anchor)],
                Some(anchor),
                false,
                false,
            ))
            .expect("drag back to anchor succeeds");
        assert_eq!(controller.camera(), Some(before));
    }

    #[test]
    fn several_moves_before_the_first_update_keep_the_down_anchor() {
        let mut controller = expanded_controller();
        let before = at_step(&mut controller, 74);
        let anchor = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let final_position = PointerPosition {
            x: 52 * 256,
            y: 40 * 256,
        };
        controller
            .update(&pointer_update(
                vec![
                    PointerEvent::Down(anchor),
                    PointerEvent::Move(PointerPosition {
                        x: 43 * 256,
                        y: anchor.y,
                    }),
                    PointerEvent::Move(PointerPosition {
                        x: 47 * 256,
                        y: anchor.y,
                    }),
                    PointerEvent::Move(final_position),
                    PointerEvent::Up(final_position),
                ],
                Some(final_position),
                false,
                false,
            ))
            .expect("whole gesture succeeds");
        let after = controller.camera().expect("camera remains open");
        assert_eq!(
            after.center.x,
            before.center.x - 3 * STEP_SIZE[usize::from(before.step)]
        );
    }

    #[test]
    fn cursor_world_pixel_stays_fixed_at_steps_zero_fifteen_forty_and_seventy_four() {
        let mut controller = expanded_controller_with(401, 321, 24);
        for step in [0, 15, 40, 74] {
            for cursor in [
                PointerPosition {
                    x: 160 * 256,
                    y: 120 * 256,
                },
                PointerPosition {
                    x: 200 * 256,
                    y: 160 * 256,
                },
                PointerPosition {
                    x: 240 * 256,
                    y: 200 * 256,
                },
            ] {
                let before = at_step(&mut controller, step);
                let world = world_pixel_under(before, cursor);
                controller
                    .update(&pointer_update_in_view(
                        401,
                        321,
                        vec![PointerEvent::Scroll {
                            position: cursor,
                            amount: 256,
                            unit: ScrollUnit::Lines,
                        }],
                        Some(cursor),
                        false,
                        false,
                    ))
                    .expect("anchored zoom succeeds");
                let after = controller.camera().expect("camera remains open");
                assert_eq!(after.step, step + 1);
                let new_size = i64::from(STEP_SIZE[usize::from(after.step)]);
                let cursor_x = i64::from(cursor.x).div_euclid(4 * 256);
                let cursor_y = i64::from(cursor.y).div_euclid(4 * 256);
                let top_left = after.top_left();
                assert_eq!(
                    i64::from(top_left.x) + cursor_x,
                    world.0.div_euclid(new_size),
                    "step={step}, cursor={cursor:?}, x"
                );
                assert_eq!(
                    i64::from(top_left.y) + cursor_y,
                    world.1.div_euclid(new_size),
                    "step={step}, cursor={cursor:?}, y"
                );
            }
        }
    }

    #[test]
    fn precise_scroll_uses_the_default_and_configured_point_thresholds() {
        for threshold in [24, 40] {
            let mut controller = expanded_controller_with(400, 320, threshold);
            let before = controller.camera().expect("initial camera");
            let cursor = PointerPosition {
                x: 160 * 256,
                y: 120 * 256,
            };
            assert!(
                controller
                    .update(&pointer_update(
                        vec![PointerEvent::Scroll {
                            position: cursor,
                            amount: i32::try_from(threshold * 256).expect("point threshold fits"),
                            unit: ScrollUnit::Points,
                        }],
                        Some(cursor),
                        false,
                        false,
                    ))
                    .expect("precise scroll publishes")
            );
            assert_eq!(
                controller.camera().expect("camera remains open").step,
                before.step + 1
            );
        }
    }

    #[test]
    fn scroll_uses_its_position_when_a_later_move_shares_the_snapshot() {
        let mut controller = expanded_controller_with(401, 321, 24);
        let before = at_step(&mut controller, 40);
        let anchor = PointerPosition {
            x: 160 * 256,
            y: 120 * 256,
        };
        let later = PointerPosition {
            x: 300 * 256,
            y: 220 * 256,
        };
        let world = world_pixel_under(before, anchor);

        controller
            .update(&pointer_update_in_view(
                401,
                321,
                vec![
                    PointerEvent::Scroll {
                        position: anchor,
                        amount: 256,
                        unit: ScrollUnit::Lines,
                    },
                    PointerEvent::Move(later),
                ],
                Some(later),
                false,
                false,
            ))
            .expect("scroll and later motion share one update");

        let after = controller.camera().expect("camera remains open");
        assert_eq!(after.step, before.step + 1);
        let new_size = i64::from(STEP_SIZE[usize::from(after.step)]);
        let cursor_x = i64::from(anchor.x).div_euclid(4 * 256);
        let cursor_y = i64::from(anchor.y).div_euclid(4 * 256);
        let top_left = after.top_left();
        assert_eq!(
            i64::from(top_left.x) + cursor_x,
            world.0.div_euclid(new_size)
        );
        assert_eq!(
            i64::from(top_left.y) + cursor_y,
            world.1.div_euclid(new_size)
        );
    }

    #[test]
    fn known_drag_overflow_catches_up_and_lost_down_does_not_start_a_drag() {
        let mut known = expanded_controller();
        let before = at_step(&mut known, 74);
        let anchor = PointerPosition {
            x: 40 * 256,
            y: 40 * 256,
        };
        let first = PointerPosition {
            x: 44 * 256,
            y: anchor.y,
        };
        known
            .update(&pointer_update(
                vec![PointerEvent::Down(anchor), PointerEvent::Move(first)],
                Some(first),
                true,
                false,
            ))
            .expect("initial drag moves");
        let latest = PointerPosition {
            x: 52 * 256,
            y: anchor.y,
        };
        known
            .update(&pointer_update(Vec::new(), Some(latest), false, true))
            .expect("overflow catches up and releases");
        let after = known.camera().expect("camera remains open");
        assert_eq!(
            after.center.x,
            before.center.x - 3 * STEP_SIZE[usize::from(before.step)]
        );

        let mut lost = expanded_controller();
        let before = at_step(&mut lost, 74);
        assert!(
            !lost
                .update(&pointer_update(Vec::new(), Some(latest), true, true))
                .expect("overflow without a known Down is ignored")
        );
        assert_eq!(lost.camera(), Some(before));
    }

    #[test]
    fn scroll_zoom_clamps_at_steps_zero_and_seventy_five() {
        let cursor = PointerPosition {
            x: 160 * 256,
            y: 120 * 256,
        };
        let mut low = expanded_controller();
        at_step(&mut low, 0);
        assert!(
            !low.update(&pointer_update(
                vec![PointerEvent::Scroll {
                    position: cursor,
                    amount: -256,
                    unit: ScrollUnit::Lines,
                }],
                Some(cursor),
                false,
                false
            ))
            .expect("low clamp is harmless")
        );
        assert_eq!(low.camera().expect("camera remains open").step, 0);

        let mut high = expanded_controller();
        at_step(&mut high, 75);
        assert!(
            !high
                .update(&pointer_update(
                    vec![PointerEvent::Scroll {
                        position: cursor,
                        amount: 256,
                        unit: ScrollUnit::Lines,
                    }],
                    Some(cursor),
                    false,
                    false
                ))
                .expect("high clamp is harmless")
        );
        assert_eq!(high.camera().expect("camera remains open").step, 75);
    }

    #[test]
    fn pointer_scroll_is_applied_before_keyboard_zoom_and_overlap_totals_two_steps() {
        let mut controller = expanded_controller();
        let before = at_step(&mut controller, 40);
        let cursor = PointerPosition {
            x: 180 * 256,
            y: 140 * 256,
        };
        let mut expected = before;
        zoom_at(&mut expected, 1, cursor, controller.crop);
        change_step(&mut expected, 1);
        let mut combined = pointer_update(
            vec![PointerEvent::Scroll {
                position: cursor,
                amount: 256,
                unit: ScrollUnit::Lines,
            }],
            Some(cursor),
            false,
            false,
        );
        combined.input.actions.push(ActionSnapshot {
            name: ZOOM_IN.to_owned(),
            held: true,
            pressed: true,
            released: false,
        });
        controller
            .update(&combined)
            .expect("ordered pointer and key zoom");
        assert_eq!(controller.camera(), Some(expected));

        let mut overlap = expanded_controller();
        let start = at_step(&mut overlap, 40);
        let scroll = PointerEvent::Scroll {
            position: cursor,
            amount: 256,
            unit: ScrollUnit::Lines,
        };
        let first_update = pointer_update(vec![scroll], Some(cursor), false, false);
        overlap
            .update(&first_update)
            .expect("first overlapped scroll");
        let second_update = pointer_update(vec![scroll], Some(cursor), false, false);
        overlap
            .update(&second_update)
            .expect("second overlapped scroll");
        assert_eq!(
            overlap.camera().expect("camera remains open").step,
            start.step + 2
        );
    }

    #[test]
    fn scroll_accumulates_fractional_lines_and_zoom_steps_at_each_event_position() {
        let mut controller = expanded_controller();
        let before = controller.camera().expect("initial camera");
        let cursor = PointerPosition {
            x: 100 * 256,
            y: 80 * 256,
        };
        assert!(
            !controller
                .update(&pointer_update(
                    vec![PointerEvent::Scroll {
                        position: cursor,
                        amount: 128,
                        unit: ScrollUnit::Lines
                    },],
                    Some(cursor),
                    false,
                    false
                ))
                .expect("fractional scroll is retained")
        );
        assert!(
            controller
                .update(&pointer_update(
                    vec![PointerEvent::Scroll {
                        position: cursor,
                        amount: 128,
                        unit: ScrollUnit::Lines
                    },],
                    Some(cursor),
                    false,
                    false
                ))
                .expect("one line zooms")
        );
        assert_eq!(
            controller.camera().expect("camera remains open").step,
            before.step + 1
        );
        assert!(
            !controller
                .update(&pointer_update(
                    vec![
                        PointerEvent::Scroll {
                            position: cursor,
                            amount: 128,
                            unit: ScrollUnit::Lines
                        },
                        PointerEvent::Scroll {
                            position: cursor,
                            amount: -128,
                            unit: ScrollUnit::Lines
                        },
                    ],
                    Some(cursor),
                    false,
                    false
                ))
                .expect("reversal clears line remainder")
        );
    }

    #[test]
    fn an_idle_update_does_not_publish_or_change_the_camera() {
        let mut controller = controller(4, 20);
        let before = controller.camera().expect("initial camera");
        assert!(
            !controller
                .update(&update(&[], Duration::from_millis(16)))
                .expect("idle update succeeds")
        );
        assert_eq!(controller.camera(), Some(before));
    }

    #[test]
    fn hover_tracks_marker_box_and_ends_on_reset_drag_or_outside_pointer() {
        let mut controller = hover_controller();
        let pointer = hover_pointer(&mut controller);
        controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("hover update");
        let first = controller.hover().expect("marker is hovered");
        assert_eq!(
            (first.id.as_str(), first.name.as_str()),
            ("fixture-town", "Fixture Town")
        );
        assert_eq!(first.location_index, 0);
        assert!(first.right > first.left);

        controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("unchanged cursor update");
        assert_eq!(controller.hover(), Some(first.clone()));

        let mut pan = update(&[(PAN_RIGHT, true, false)], Duration::ZERO);
        pan.input.pointer_position = Some(pointer);
        controller.update(&pan).expect("pan under cursor");
        let moved = controller.hover().expect("marker remains under cursor");
        assert_ne!(moved.left, first.left, "the tooltip box follows the camera");

        let mut reset = pointer_update(Vec::new(), Some(pointer), false, false);
        reset.input.reset = true;
        controller.update(&reset).expect("focus reset");
        assert_eq!(controller.hover(), None);

        controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("hover again");
        controller
            .update(&pointer_update(Vec::new(), Some(pointer), true, false))
            .expect("held left button");
        assert_eq!(controller.hover(), None);

        let outside = PointerPosition {
            x: 401 * 256,
            y: 10 * 256,
        };
        controller
            .update(&pointer_update(Vec::new(), Some(outside), false, false))
            .expect("outside pointer");
        assert_eq!(controller.hover(), None);
    }

    #[test]
    fn hover_box_is_in_window_points_at_two_steps_and_camera_positions() {
        let mut controller = hover_controller();
        let pointer = hover_pointer(&mut controller);
        controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("step 46 hover");
        let at_center = controller.hover().expect("marker is hovered");
        assert_eq!(
            (at_center.left, at_center.right, at_center.y),
            (192, 208, 160)
        );

        let crop = controller.crop;
        let step = 15_u8;
        let pixel = i64::from(STEP_SIZE[usize::from(step)]);
        let center = FixedPoint {
            x: i32::try_from(
                i64::from(crop.origin.x + 100) * ck_map::FIXED_PER_CELL
                    + ck_map::FIXED_PER_CELL / 2
                    + pixel * 5,
            )
            .expect("offset center fits"),
            y: i32::try_from(
                i64::from(crop.origin.y + 80) * ck_map::FIXED_PER_CELL + ck_map::FIXED_PER_CELL / 2
                    - pixel * 3,
            )
            .expect("offset center fits"),
        };
        controller.camera = Some(
            Camera::new(step, center, ViewSize::from_points(400, 320), crop)
                .expect("offset camera is valid"),
        );
        let pointer = hover_pointer(&mut controller);
        controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("step 15 hover");
        let offset = controller.hover().expect("marker is hovered at step 15");
        assert_eq!((offset.left, offset.right, offset.y), (172, 188, 172));

        let mut odd_controller = hover_controller_with_glyph(vec![
            vec!['.', '#', '.'],
            vec!['#', 'd', '#'],
            vec!['.', '#', '.'],
        ]);
        let pointer = hover_pointer(&mut odd_controller);
        odd_controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("step 46 odd-glyph hover");
        let odd_at_center = odd_controller.hover().expect("odd marker is hovered");
        assert_eq!(
            (odd_at_center.left, odd_at_center.right, odd_at_center.y),
            (192, 212, 162)
        );

        let crop = odd_controller.crop;
        let step = 15_u8;
        let pixel = i64::from(STEP_SIZE[usize::from(step)]);
        let center = FixedPoint {
            x: i32::try_from(
                i64::from(crop.origin.x + 100) * ck_map::FIXED_PER_CELL
                    + ck_map::FIXED_PER_CELL / 2
                    + pixel * 5,
            )
            .expect("odd offset center fits"),
            y: i32::try_from(
                i64::from(crop.origin.y + 80) * ck_map::FIXED_PER_CELL + ck_map::FIXED_PER_CELL / 2
                    - pixel * 3,
            )
            .expect("odd offset center fits"),
        };
        odd_controller.camera = Some(
            Camera::new(step, center, ViewSize::from_points(400, 320), crop)
                .expect("odd offset camera is valid"),
        );
        let pointer = hover_pointer(&mut odd_controller);
        odd_controller
            .update(&pointer_update(Vec::new(), Some(pointer), false, false))
            .expect("step 15 odd-glyph hover");
        let odd_offset = odd_controller
            .hover()
            .expect("odd marker is hovered at step 15");
        assert_eq!(
            (odd_offset.left, odd_offset.right, odd_offset.y),
            (172, 192, 174)
        );
    }

    #[test]
    fn a_window_opened_in_pixels_gives_the_map_the_resolved_points_view() {
        // 1600x900 pixels at 2 pixels a point resolve to 800x450 points; the map's initial camera is the one a points open of
        // 800x450 gives (not square, so a width taken for a height would show).
        let resolved = WindowDesc {
            title: "pixels".to_owned(),
            unit: ck_window::SizeUnit::Pixels,
            width: 1600,
            height: 900,
            resizable: true,
            ..WindowDesc::default()
        }
        .resolve(&ck_window::ScreenSnapshot {
            backing: 2.0,
            visible_points: (4000, 3000),
        })
        .expect("resolves")
        .desc;
        assert_eq!((resolved.width, resolved.height), (800, 450));
        let open = |desc: &WindowDesc| {
            let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
            let mut controller = MapController::new(&loaded_map(), bridge, 4, 20, 24)
                .expect("map lane accepts fixture");
            controller.open(desc).expect("initial view is valid");
            controller.camera().expect("a camera")
        };
        let points = WindowDesc {
            title: "points".to_owned(),
            width: 800,
            height: 450,
            ..WindowDesc::default()
        };
        assert_eq!(open(&resolved), open(&points));
        assert_eq!(open(&resolved).view, ViewSize::from_points(800, 450));
    }

    #[test]
    fn a_change_of_the_metrics_generation_alone_republishes_the_camera_and_the_bridge_adopts_it() {
        let (mut controller, bridge, sink) = river_test_controller();
        let opened = controller.camera().expect("a camera");
        let first = sink.latest().map.expect("the opening camera was adopted");
        // 4 points a map pixel at 2 pixels a point: 8 drawable pixels each.
        assert_eq!(
            (
                first.placement.expect("placed").destination.right,
                first.placement.expect("placed").destination.bottom
            ),
            (800, 640)
        );
        // A move to a 1x display: the points stay 400x320 and only the pixels move. The bridge's metrics are a newer generation, so
        // the old camera is no longer placed.
        let generation = bridge.metrics_generation();
        bridge.set_metrics(
            (400, 320),
            PixelSize {
                width: 400,
                height: 320,
            },
        );
        assert_eq!(bridge.metrics_generation(), generation + 1);
        assert!(sink.latest().map.is_none(), "a stale camera is not drawn");
        // The next update, with the view, pan and zoom unchanged, publishes the camera again for the new generation.
        let mut tick = update(&[], Duration::ZERO);
        tick.metrics.pixels = PixelSize {
            width: 400,
            height: 320,
        };
        assert_eq!(controller.update(&tick), Ok(true), "republished");
        assert_eq!(controller.camera(), Some(opened), "the view is unchanged");
        let after = sink.latest().map.expect("the camera was adopted");
        assert_eq!(after.camera.metrics_generation(), generation + 1);
        assert_eq!(
            (
                after.placement.expect("placed").destination.right,
                after.placement.expect("placed").destination.bottom
            ),
            (400, 320),
            "a map pixel is four drawable pixels at 1x: still four points"
        );
        // Nothing changed since: nothing is published again.
        assert_eq!(controller.update(&tick), Ok(false));
    }
}
