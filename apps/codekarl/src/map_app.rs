//! The application's single owner of the map camera and its paced keyboard controls.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ck_map::{
    Camera, Crop, FixedPoint, LoadedMap, PreparedRivers, STEP_SIZE, TerrainKind, ViewSize,
    WorldPixel,
};
use ck_render_2d::{
    MapCamera, MapInput, MapPublication, MapRules, MapTerrainKind, MapTerrainRule, SceneBridge,
};
use ck_window::{ActionSnapshot, UpdateEvent, WindowDesc};

const MAP_INPUT_GENERATION: u64 = 1;
const PAN_LEFT: &str = "map_pan_left";
const PAN_RIGHT: &str = "map_pan_right";
const PAN_UP: &str = "map_pan_up";
const PAN_DOWN: &str = "map_pan_down";
const ZOOM_IN: &str = "map_zoom_in";
const ZOOM_OUT: &str = "map_zoom_out";

pub(crate) type SharedController = Arc<Mutex<MapController>>;

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
    pan_pixels_per_frame: i32,
    zoom_steps_per_second: u32,
    camera: Option<Camera>,
    zoom_in_elapsed: Duration,
    zoom_out_elapsed: Duration,
}

impl MapController<PreparedRivers> {
    pub(crate) fn new(
        loaded: &LoadedMap,
        bridge: Arc<SceneBridge>,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
    ) -> Result<Self, String> {
        Self::with_rivers(
            loaded,
            bridge,
            pan_pixels_per_frame,
            zoom_steps_per_second,
            PreparedRivers::new(loaded.rivers.clone()),
        )
    }
}

impl<R: RiverViewSelector> MapController<R> {
    fn with_rivers(
        loaded: &LoadedMap,
        bridge: Arc<SceneBridge>,
        pan_pixels_per_frame: u32,
        zoom_steps_per_second: u32,
        rivers: R,
    ) -> Result<Self, String> {
        let crop = loaded.data.crop;
        let rules = rules(loaded);
        let input = MapInput::new(
            MAP_INPUT_GENERATION,
            [crop.origin.x, crop.origin.y],
            [crop.width, crop.height],
            flatten(&loaded.grid_a.pixels),
            flatten(&loaded.grid_b.pixels),
            rules,
        )
        .map_err(|error| format!("map lane refused the loaded map: {error}"))?;
        let input = Arc::new(input);
        match bridge.publish_map(Arc::clone(&input)) {
            MapPublication::Published => {}
            outcome => return Err(format!("map lane refused the loaded map: {outcome:?}")),
        }
        Ok(Self {
            bridge,
            crop,
            rivers,
            pan_pixels_per_frame: i32::try_from(pan_pixels_per_frame)
                .map_err(|_| "map pan_pixels_per_frame does not fit i32".to_owned())?,
            zoom_steps_per_second,
            camera: None,
            zoom_in_elapsed: Duration::ZERO,
            zoom_out_elapsed: Duration::ZERO,
        })
    }

    /// Set up the initial view from the exact window description before the request reaches the backend.
    pub(crate) fn open(&mut self, desc: &WindowDesc) -> Result<(), String> {
        let view = ViewSize::from_points(desc.width, desc.height);
        let camera = initial_camera(self.crop, view)?;
        let snapshot = self.snapshot(camera, self.bridge.metrics_generation().saturating_add(1))?;
        self.camera = Some(camera);
        match self.bridge.stage_initial_camera(Arc::new(snapshot)) {
            MapPublication::Published => Ok(()),
            outcome => Err(format!("map lane refused the initial camera: {outcome:?}")),
        }
    }

    /// Apply one paced input snapshot, publishing only when the camera or metrics changed.
    pub(crate) fn update(&mut self, update: &UpdateEvent) -> Result<bool, String> {
        let Some(mut camera) = self.camera else {
            return Ok(false);
        };
        let view = ViewSize::from_points(update.metrics.points.0, update.metrics.points.1);
        let metrics_generation = self.bridge.metrics_generation();
        let mut changed = false;
        if camera.view != view {
            camera.resize(view);
            changed = true;
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

        if changed || self.bridge.metrics_generation() != metrics_generation {
            self.camera = Some(camera);
            self.publish_for_generation(camera, self.bridge.metrics_generation())?;
            return Ok(true);
        }
        Ok(false)
    }

    fn snapshot(&mut self, camera: Camera, generation: u64) -> Result<MapCamera, String> {
        let top_left = camera.top_left();
        let rivers = self
            .rivers
            .select_view(camera.step, top_left, camera.view)
            .ok_or_else(|| format!("map river coverage is invalid for step {}", camera.step))?;
        MapCamera::new(
            camera.step,
            [top_left.x, top_left.y],
            [camera.view.width, camera.view.height],
            rivers,
            generation,
        )
        .map_err(|error| format!("map camera is invalid: {error}"))
    }

    fn publish_for_generation(&mut self, camera: Camera, generation: u64) -> Result<(), String> {
        let snapshot = self.snapshot(camera, generation)?;
        match self.bridge.publish_camera(Arc::new(snapshot)) {
            MapPublication::Published => Ok(()),
            outcome => Err(format!("map lane refused the camera snapshot: {outcome:?}")),
        }
    }

    #[cfg(test)]
    fn camera(&self) -> Option<Camera> {
        self.camera
    }
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

fn rules(loaded: &LoadedMap) -> MapRules {
    let terrains = loaded
        .terrain_types
        .iter()
        .map(|terrain| {
            let palette = rgba(terrain.palette_colour);
            let style = terrain
                .style_index
                .map(|index| &loaded.data.terrains[usize::from(index)]);
            let (
                solid,
                second,
                dark,
                dark_second,
                cap,
                high_ground,
                relief,
                mix_percent,
                cap_heights,
            ) = style.map_or(
                (
                    palette,
                    palette,
                    palette,
                    palette,
                    palette,
                    palette,
                    [0, 1],
                    0,
                    [0, 1],
                ),
                |style| {
                    (
                        rgba(style.solid),
                        rgba(style.second),
                        rgba(style.dark),
                        rgba(style.dark_second),
                        rgba(style.cap),
                        rgba(style.high_ground),
                        [style.relief_low, style.relief_high],
                        style.mix_percent,
                        [style.cap_low, style.cap_high],
                    )
                },
            );
            let kind = match terrain.kind {
                TerrainKind::Land => MapTerrainKind::Land,
                TerrainKind::Sea => MapTerrainKind::Sea,
                TerrainKind::Lake => MapTerrainKind::Lake,
                TerrainKind::Mountain => MapTerrainKind::Mountain,
            };
            MapTerrainRule {
                kind,
                solid,
                second,
                dark,
                dark_second,
                cap,
                high_ground,
                relief,
                mix_percent,
                cap_heights,
            }
        })
        .collect();
    MapRules {
        terrains,
        sea: rgba(loaded.data.sea),
        shallow_sea: rgba(loaded.data.shallow_sea),
        lake: rgba(loaded.data.lake),
        river: rgba(loaded.data.river),
        shallow_threshold: loaded.data.shallow_threshold,
        steps: STEP_SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

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
                pixels: vec![[0, 0, 0, 255]; 4],
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

    fn controller(pan: u32, zoom: u32) -> MapController {
        let loaded = loaded_map();
        let bridge = SceneBridge::new(ck_render_2d::Filter::Nearest);
        let mut controller =
            MapController::new(&loaded, bridge, pan, zoom).expect("map lane accepts fixture");
        controller
            .open(&WindowDesc {
                title: "map controller test".to_owned(),
                width: 400,
                height: 320,
                clear: [0.0; 3],
            })
            .expect("initial view is valid");
        controller
    }

    fn update(actions: &[(&str, bool, bool)], dt: Duration) -> UpdateEvent {
        UpdateEvent {
            sequence: 1,
            elapsed: dt,
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
            },
        }
    }

    fn at_step<R: RiverViewSelector>(controller: &mut MapController<R>, step: u8) -> Camera {
        let current = controller.camera.expect("opened camera");
        let camera = Camera::new(step, current.center, current.view, controller.crop)
            .expect("fixture camera");
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
            CountingRivers::new(loaded.rivers.clone()),
        )
        .expect("map lane accepts fixture");
        let desc = WindowDesc {
            title: "river controller test".to_owned(),
            width: 400,
            height: 320,
            clear: [0.0; 3],
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
}
