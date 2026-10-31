//! The assets rule, on real directories and links.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ck_resources::{Assets, AssetsError, DecodeError};

use crate::fixtures::fixture;

/// A scratch directory, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ck-render-2d-assets-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&path).expect("a scratch directory");
        // Compare real paths: the temporary directory is itself a link on macOS.
        Self(path.canonicalize().expect("a canonical scratch directory"))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// A directory called `name`.
    fn directory(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        fs::create_dir_all(&path).expect("a directory");
        path
    }

    /// A PNG at `path` (relative to the scratch directory), made from the fixture `from`.
    fn png(&self, path: &str, from: &str) -> PathBuf {
        let path = self.path(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        fs::write(&path, fixture(from)).expect("a file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_name_refused(assets: &Assets, name: &str, fragment: &str) {
    match assets.open(name) {
        Err(AssetsError::Name {
            name: refused,
            reason,
        }) => {
            assert_eq!(refused, name);
            assert!(reason.contains(fragment), "{name:?}: {reason}");
            let message = AssetsError::Name {
                name: refused,
                reason,
            }
            .to_string();
            assert!(message.contains(&format!("{name:?}")), "{message}");
        }
        other => panic!("{name:?} was not refused by name: {other:?}"),
    }
}

#[test]
fn nested_names_open_and_decode() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    scratch.png("assets/top.png", "rgba8");
    scratch.png("assets/sprites/hero/walk.png", "grey8");
    let assets = Assets::new(&root);
    assert_eq!(assets.root(), root);
    assert_eq!(assets.open("top.png").unwrap().width(), 2);
    let nested = assets.open("sprites/hero/walk.png").unwrap();
    assert_eq!((nested.width(), nested.height()), (4, 1));
    // A repeated separator and a trailing one are still normal components.
    assert!(assets.open("sprites//hero/walk.png").is_ok());
}

#[test]
fn names_that_are_not_relative_normal_paths_are_refused_by_name() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    let inside = scratch.png("assets/inside.png", "rgba8");
    scratch.png("outside.png", "rgba8");
    let assets = Assets::new(&root);
    assert_name_refused(&assets, "", "empty");
    assert_name_refused(&assets, ".", "`.`");
    assert_name_refused(&assets, "./inside.png", "`.`");
    assert_name_refused(&assets, "sprites/./inside.png", "`.`");
    assert_name_refused(&assets, "..", "`..`");
    assert_name_refused(&assets, "../outside.png", "`..`");
    assert_name_refused(&assets, "sprites/../inside.png", "`..`");
    assert_name_refused(&assets, "a/../../outside.png", "`..`");
    // An absolute name is refused even when it names a file inside the root.
    assert_name_refused(
        &assets,
        inside.to_str().expect("a UTF-8 path"),
        "not relative",
    );
    assert_name_refused(&assets, "/etc/hosts", "not relative");
    // A Windows prefix is not a prefix here, but a drive-looking name is a plain (missing) file.
    assert!(matches!(
        assets.open("C:/x.png"),
        Err(AssetsError::Open { .. })
    ));
}

#[test]
fn a_link_that_stays_inside_the_root_is_allowed() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    scratch.png("assets/real/sprite.png", "rgba8");
    symlink(
        scratch.path("assets/real/sprite.png"),
        scratch.path("assets/file-link.png"),
    )
    .unwrap();
    symlink("real", scratch.path("assets/dir-link")).unwrap();
    let assets = Assets::new(&root);
    assert!(assets.open("file-link.png").is_ok());
    assert!(assets.open("dir-link/sprite.png").is_ok());
}

#[test]
fn a_file_link_or_a_directory_link_that_leaves_the_root_is_refused() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    let outside = scratch.png("elsewhere/secret.png", "rgba8");
    symlink(&outside, scratch.path("assets/file-link.png")).unwrap();
    symlink(scratch.path("elsewhere"), scratch.path("assets/dir-link")).unwrap();
    // A relative link out of the root, and a link that only passes through the outside.
    symlink(
        "../elsewhere/secret.png",
        scratch.path("assets/relative-link.png"),
    )
    .unwrap();
    fs::create_dir_all(scratch.path("assets/real")).unwrap();
    symlink("../../elsewhere", scratch.path("assets/real/up")).unwrap();
    let assets = Assets::new(&root);
    for name in [
        "file-link.png",
        "dir-link/secret.png",
        "relative-link.png",
        "real/up/secret.png",
    ] {
        match assets.open(name) {
            Err(AssetsError::Outside {
                name: refused,
                resolved,
            }) => {
                assert_eq!(refused, name);
                assert_eq!(resolved, outside.canonicalize().unwrap());
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    // A sibling directory whose name starts with the root's is not inside it.
    let sibling = scratch.png("assets-extra/x.png", "rgba8");
    symlink(&sibling, scratch.path("assets/sibling.png")).unwrap();
    assert!(matches!(
        assets.open("sibling.png"),
        Err(AssetsError::Outside { .. })
    ));
}

#[test]
fn a_missing_root_is_an_error_at_the_first_open_and_at_every_later_one() {
    let scratch = Scratch::new();
    let root = scratch.path("assets");
    // Construction succeeds with nothing there.
    let assets = Assets::new(&root);
    for _ in 0..3 {
        match assets.open("anything.png") {
            Err(AssetsError::Root {
                root: named,
                reason,
            }) => {
                assert_eq!(named, root);
                assert!(!reason.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }
    let message = assets.open("x.png").unwrap_err().to_string();
    assert!(message.contains(root.to_str().unwrap()), "{message}");
    // The error was kept: creating the directory afterwards does not change this assets object.
    scratch.png("assets/anything.png", "rgba8");
    assert!(matches!(
        assets.open("anything.png"),
        Err(AssetsError::Root { .. })
    ));
    // A new object finds it.
    assert!(Assets::new(&root).open("anything.png").is_ok());
}

#[test]
fn a_root_that_is_a_file_is_unusable() {
    let scratch = Scratch::new();
    let file = scratch.png("not-a-directory", "rgba8");
    let error = Assets::new(&file).open("x.png").unwrap_err();
    assert!(
        matches!(&error, AssetsError::Root { reason, .. } if reason.contains("directory")),
        "{error}"
    );
}

#[test]
fn the_root_is_resolved_once_at_the_first_open() {
    let scratch = Scratch::new();
    scratch.png("first/a.png", "rgba8");
    scratch.png("second/a.png", "grey8");
    let link = scratch.path("current");
    symlink("first", &link).unwrap();
    let assets = Assets::new(&link);
    assert_eq!(assets.open("a.png").unwrap().width(), 2, "the first root");
    // Repoint the link: the canonical root was kept, so it is still the first.
    fs::remove_file(&link).unwrap();
    symlink("second", &link).unwrap();
    assert_eq!(assets.open("a.png").unwrap().width(), 2);
    // A fresh object resolves afresh.
    assert_eq!(Assets::new(&link).open("a.png").unwrap().width(), 4);
}

#[test]
fn a_file_that_is_missing_unreadable_or_not_a_png_is_an_error_naming_it() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    fs::write(scratch.path("assets/text.png"), b"hello").unwrap();
    fs::write(scratch.path("assets/cut.png"), &fixture("rgba8")[..40]).unwrap();
    fs::create_dir_all(scratch.path("assets/folder.png")).unwrap();
    let assets = Assets::new(&root);
    match assets.open("missing.png") {
        Err(AssetsError::Open { name, source }) => {
            assert_eq!(name, "missing.png");
            assert_eq!(source.kind(), io::ErrorKind::NotFound);
        }
        other => panic!("{other:?}"),
    }
    match assets.open("text.png") {
        Err(AssetsError::Decode { name, source }) => {
            assert_eq!((name.as_str(), source), ("text.png", DecodeError::NotPng));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        assets.open("cut.png"),
        Err(AssetsError::Decode {
            source: DecodeError::Truncated,
            ..
        })
    ));
    assert!(matches!(
        assets.open("folder.png"),
        Err(AssetsError::NotAFile { .. })
    ));
    let message = assets.open("text.png").unwrap_err().to_string();
    assert!(
        message.contains("text.png") && message.contains("not a PNG"),
        "{message}"
    );
}

#[test]
fn the_file_is_opened_by_its_canonical_path_and_decoded_from_the_handle() {
    // A name with a link in the middle resolves to the canonical file; the decode sees that file's bytes.
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    scratch.png("assets/store/a.png", "grey8");
    symlink("store", scratch.path("assets/alias")).unwrap();
    let image = Assets::new(&root).open("alias/a.png").unwrap();
    assert_eq!((image.width(), image.height()), (4, 1));
}

/// `check_name` is the name rules of `open` alone: the same accepted and refused names, the same messages, and no file
/// system (the root does not exist).
#[test]
fn check_name_refuses_what_open_refuses_by_name_and_touches_no_file_system() {
    let refused: [(&str, &str); 8] = [
        ("", "empty"),
        (".", "`.`"),
        ("./inside.png", "`.`"),
        ("sprites/./inside.png", "`.`"),
        ("..", "`..`"),
        ("../outside.png", "`..`"),
        ("a/../../outside.png", "`..`"),
        ("/etc/hosts", "not relative"),
    ];
    let scratch = Scratch::new();
    let missing_root = scratch.path("never-created");
    let assets = Assets::new(&missing_root);
    for (name, fragment) in refused {
        let error = Assets::check_name(name).unwrap_err();
        match &error {
            AssetsError::Name {
                name: refused,
                reason,
            } => {
                assert_eq!(refused, name);
                assert!(reason.contains(fragment), "{name:?}: {reason}");
            }
            other => panic!("{name:?}: {other:?}"),
        }
        // With the root missing, `open` reports the root first (its existing precedence); once the root exists it
        // reports exactly the error `check_name` gave.
        assert!(matches!(assets.open(name), Err(AssetsError::Root { .. })));
        let root = scratch.directory(&format!("root-{}", name.len()));
        let usable = Assets::new(&root);
        assert_eq!(
            usable.open(name).unwrap_err().to_string(),
            error.to_string(),
            "{name:?}: the same message from open"
        );
    }
    for name in [
        "top.png",
        "sprites/hero/walk.png",
        "sprites//hero/walk.png",
        "C:/x.png",
        "a b.png",
        "ünïcode.png",
    ] {
        assert!(Assets::check_name(name).is_ok(), "{name:?}");
    }
    assert!(!missing_root.exists(), "nothing was created or looked at");
}

/// `open_reserving` is `open` with a reservation: the same image, the reservation asked for the decoded size, a refusal
/// stops it, and the root is resolved at its first call.
#[test]
fn open_reserving_is_open_with_a_reservation() {
    let scratch = Scratch::new();
    let root = scratch.directory("assets");
    scratch.png("assets/top.png", "rgba8");
    let assets = Assets::new(&root);
    let mut asked = Vec::new();
    let (image, granted) = assets
        .open_reserving("top.png", &ck_resources::DecodeLimits::SHIPPED, |size| {
            asked.push(size);
            Ok::<u64, ()>(size)
        })
        .expect("opened");
    let plain = assets.open("top.png").expect("opened");
    assert_eq!(image.pixels(), plain.pixels());
    assert_eq!(asked, vec![granted]);
    assert_eq!(
        granted,
        u64::from(image.width()) * u64::from(image.height()) * 4
    );
    assert!(matches!(
        assets.open_reserving("top.png", &ck_resources::DecodeLimits::SHIPPED, |_| Err::<
            (),
            _,
        >(
            "no"
        )),
        Err(ck_resources::OpenReserveError::Refused("no"))
    ));
    // The assets rule's errors are the usual ones, and the reservation is never asked for them.
    let mut asked = false;
    for (name, matcher) in [("missing.png", "Open"), ("../x.png", "Name")] {
        let error = assets
            .open_reserving(name, &ck_resources::DecodeLimits::SHIPPED, |_| {
                asked = true;
                Ok::<(), ()>(())
            })
            .unwrap_err();
        match (matcher, error) {
            ("Open", ck_resources::OpenReserveError::Assets(AssetsError::Open { .. }))
            | ("Name", ck_resources::OpenReserveError::Assets(AssetsError::Name { .. })) => {}
            (_, other) => panic!("{name}: {other:?}"),
        }
    }
    assert!(!asked);
    // A missing root is the usual error, kept for every later call.
    let never = Assets::new(scratch.path("never"));
    for _ in 0..2 {
        assert!(matches!(
            never.open_reserving("x.png", &ck_resources::DecodeLimits::SHIPPED, |_| Ok::<
                (),
                (),
            >(
                ()
            )),
            Err(ck_resources::OpenReserveError::Assets(
                AssetsError::Root { .. }
            ))
        ));
    }
}
