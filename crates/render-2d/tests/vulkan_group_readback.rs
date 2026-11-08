//! Scene groups on Vulkan, off screen (milestone 8, slice 4): the scenes of `group_readback` drawn through the scene presenter's own
//! frame building and drawing (`readback_scene`) under `VK_LAYER_KHRONOS_validation` with synchronization validation, in both texture
//! binding modes, and compared with the CPU reference pixel for pixel, then checked for what each frame means (see the Metal target).
//!
//! No window is opened; the target needs the GPU. An empty validation log is required from the instance's creation to its
//! destruction.

#![cfg(all(target_os = "macos", feature = "vulkan"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use ck_gpu_vulkan::{
    BindingChoice, BindlessLimits, CheckerProgress, DestinationRect, InstanceOptions, InstanceRole,
    LedgerCleanup, LedgerEvent, LedgerLimits, PixelSource, TextureLedger, TextureLimits,
    TextureTestHooks, TextureWakes, VulkanContext, VulkanFilter, VulkanFrame, VulkanInstance,
    VulkanSceneSource, checker_pixels, create_instance_with, destroy_surface_and_instance,
    load_loader, readback_scene, vk,
};

mod api {
    pub(crate) use ck_render_2d::*;
}

#[path = "group_readback/mod.rs"]
mod group_readback;

use group_readback::{CLEAR, Instance, Picture, SIZE, check, scenes};

const PATIENCE: Duration = Duration::from_secs(30);
const SLOTS: u32 = 131_072;

struct Bytes(Vec<u8>);

impl PixelSource for Bytes {
    fn pixels(&self) -> &[u8] {
        &self.0
    }
}

/// The instances of one frame, as the scene source the presenter's frame building takes.
struct Quads(Vec<VulkanInstance<u32>>);

impl VulkanSceneSource<u32> for Quads {
    fn place(&self, _width: u32, _height: u32, out: &mut dyn FnMut(VulkanInstance<u32>)) {
        for instance in &self.0 {
            out(*instance);
        }
    }
}

/// An instance with validation, a device context, its queue and a ledger whose pictures are shown.
struct Rig {
    instance: Option<ck_gpu_vulkan::Instance>,
    log: ck_gpu_vulkan::ValidationLogHandle,
    context: Arc<VulkanContext>,
    queue: Option<ck_gpu_vulkan::RenderQueue>,
    ledger: Option<TextureLedger<u32>>,
}

impl Rig {
    fn new(pictures: &[Picture], choice: BindingChoice) -> Self {
        let loader = load_loader().expect("the Vulkan loader loads");
        let instance = create_instance_with(
            &loader,
            &[],
            InstanceOptions {
                validation: true,
                synchronization: true,
            },
        )
        .expect("a validated headless instance");
        let log = instance.validation_log();
        let context = VulkanContext::new(&instance, None).expect("a device context");
        let queue = context.take_queue().expect("the queue is handed out once");
        let ledger = TextureLedger::<u32>::start_with_binding(
            context.clone(),
            TextureLimits::default(),
            LedgerLimits::default(),
            16,
            choice,
            TextureWakes::default(),
            TextureTestHooks::default(),
        )
        .expect("the ledger starts");
        ledger
            .start_checker(64, 64, Arc::new(Bytes(checker_pixels())))
            .expect("the checker is offered");
        let mut rig = Self {
            instance: Some(instance),
            log,
            context,
            queue: Some(queue),
            ledger: Some(ledger),
        };
        rig.pump_until(|ledger, _| ledger.checker_progress() == CheckerProgress::Resident);
        for (index, picture) in pictures.iter().enumerate() {
            rig.show(u32::try_from(index).expect("a few pictures"), picture);
        }
        rig
    }

    fn pump_until(
        &mut self,
        mut done: impl FnMut(&TextureLedger<u32>, &[LedgerEvent<u32>]) -> bool,
    ) {
        let mut events = Vec::new();
        let started = Instant::now();
        loop {
            let ledger = self.ledger.as_ref().expect("the ledger exists");
            ledger.pump(self.queue.as_mut().expect("the queue exists"), &mut events);
            if done(ledger, &events) {
                return;
            }
            assert!(started.elapsed() < PATIENCE, "the condition never held");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Shows `picture` as the first revision of `key`.
    fn show(&mut self, key: u32, picture: &Picture) {
        let pixels = Arc::new(Bytes(picture.bytes.clone()));
        self.ledger
            .as_ref()
            .expect("the ledger exists")
            .offer(
                key,
                1,
                picture.width,
                picture.height,
                Arc::clone(&pixels) as Arc<dyn PixelSource>,
            )
            .expect("the offer is admitted");
        self.pump_until(|_, events| {
            events.iter().any(
                |event| matches!(event, LedgerEvent::Prepared { key: k, revision: 1 } if *k == key),
            )
        });
        let ledger = self.ledger.as_ref().expect("the ledger exists");
        ledger.commit(key, 1).expect("commit");
        let mut adopted = Vec::new();
        loop {
            let before = adopted.len();
            ledger.adopt(256, &mut adopted);
            if adopted.len() == before {
                break;
            }
        }
    }

    fn draw(&mut self, instances: &[Instance]) -> Vec<u8> {
        let placed: Vec<VulkanInstance<u32>> = instances
            .iter()
            .map(|instance| VulkanInstance {
                key: u32::try_from(instance.image).expect("a few pictures"),
                destination: DestinationRect {
                    left: instance.destination[0],
                    top: instance.destination[1],
                    right: instance.destination[2],
                    bottom: instance.destination[3],
                },
                source: instance.source,
                role: InstanceRole::Scene,
            })
            .collect();
        let frame = VulkanFrame::new(
            1,
            VulkanFilter::Nearest,
            None,
            Some(Arc::new(Quads(placed))),
        );
        let (pixels, counters) = readback_scene(
            &self.context,
            self.queue.as_mut().expect("the queue exists"),
            self.ledger.as_ref().expect("the ledger exists"),
            SIZE,
            CLEAR,
            &frame,
            64,
        )
        .expect("the scene reads back");
        assert_eq!(
            counters.instances,
            u64::try_from(instances.len()).expect("a few instances"),
            "every instance drew"
        );
        assert_eq!(counters.skipped, 0);
        assert_eq!(
            counters.checker, 0,
            "every picture was shown, so no checker"
        );
        pixels
    }

    fn finish(mut self) {
        let ledger = self.ledger.take().expect("the ledger exists");
        assert_eq!(
            ledger.shutdown(Instant::now() + PATIENCE),
            LedgerCleanup::Proven,
            "the end is proven"
        );
        drop(ledger);
        drop(self.queue.take());
        let Self {
            instance,
            log,
            context,
            ..
        } = self;
        assert_eq!(
            Arc::strong_count(&context),
            1,
            "something still holds the context: the device would not be destroyed"
        );
        drop(context);
        let instance = instance.expect("the instance exists");
        // SAFETY: there is no surface (null) and no presenter ever used the instance.
        let outcome =
            unsafe { destroy_surface_and_instance(instance, vk::SurfaceKHR::null(), None) };
        if let Err(refused) = outcome {
            panic!("the instance was not destroyed: {refused}");
        }
        assert_eq!(
            log.messages(),
            Vec::<String>::new(),
            "validation reported something"
        );
    }
}

fn run(choice: BindingChoice) {
    let scenes = scenes();
    let mut rig = Rig::new(&scenes.pictures, choice);
    check(&scenes, &mut |_, instances| rig.draw(instances));
    rig.finish();
}

/// Both binding modes, one after the other, in the target's **only** test.
///
/// The validation layer keeps a process-global map that its create and destroy calls update without a lock, so two cases that make
/// instances on parallel test threads can crash inside the layer (CK-166; `ck-gpu-vulkan`'s tests take a process-wide gate for it).
/// This target therefore has exactly one `#[test]`, and the modes run in sequence inside it, each making and destroying its own
/// instance before the next begins: no two instances ever exist in this process at once, with no gate and no thread setting.
#[test]
fn groups_read_back_on_vulkan_exactly_in_both_binding_modes() {
    run(BindingChoice::PerRevision);
    run(BindingChoice::Auto(BindlessLimits {
        texture_slots: SLOTS,
    }));
}
