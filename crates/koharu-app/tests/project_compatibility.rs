//! Opt-in checks against local project metadata. Source projects are never opened:
//! only scene.bin, project.toml and history.log are copied into temporary folders.
//! Set KOHARU_COMPAT_PROJECTS to an OS-separated list of project directories, then
//! run this test with --ignored --nocapture. No image/model assets are needed.

use std::path::Path;

use camino::Utf8PathBuf;
use koharu_app::ProjectSession;
use koharu_core::{
    Node, NodeId, NodeKind, Op, Page, TextData, TextDirection, TextRangeStyle, TextStyleRange,
    Transform,
};

fn scene_bytes(session: &ProjectSession) -> Vec<u8> {
    postcard::to_allocvec(&session.scene_snapshot()).unwrap()
}

#[test]
fn historical_git_generated_snapshots_preserve_every_field() {
    // These bytes are produced from git revisions of koharu-core, not from the
    // current decoder's compat structs. The paired historical JSON independently
    // describes all stored values; current JSON defaults supply appended fields.
    for (name, bytes, expected) in [
        (
            "v1",
            include_bytes!("fixtures/scene-v1.bin").as_slice(),
            include_bytes!("fixtures/scene-v1.json").as_slice(),
        ),
        (
            "v6",
            include_bytes!("fixtures/scene-v6.bin").as_slice(),
            include_bytes!("fixtures/scene-v6.json").as_slice(),
        ),
    ] {
        let expected: koharu_core::Scene = serde_json::from_slice(expected).unwrap();
        let expected = postcard::to_allocvec(&expected).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(temp.path().join("golden.khrproj")).unwrap();
        drop(ProjectSession::create(&path, name).unwrap());
        std::fs::write(path.join("scene.bin"), bytes).unwrap();
        let session = ProjectSession::open(&path).unwrap();
        assert_eq!(session.epoch(), 42, "{name}");
        assert_eq!(scene_bytes(&session), expected, "{name} migration");
        session.compact().unwrap();
        drop(session);
        let session = ProjectSession::open(&path).unwrap();
        assert_eq!(session.epoch(), 42, "{name}");
        assert_eq!(scene_bytes(&session), expected, "{name} compaction");
    }
}

fn check_copy(source: &Path) {
    let temp = tempfile::tempdir().unwrap();
    let copy = Utf8PathBuf::from_path_buf(temp.path().join("copy.khrproj")).unwrap();
    std::fs::create_dir(&copy).unwrap();
    let mut originals = Vec::new();
    for name in ["scene.bin", "project.toml", "history.log"] {
        let path = source.join(name);
        if path.exists() {
            let bytes = std::fs::read(&path).unwrap();
            std::fs::write(copy.join(name), &bytes).unwrap();
            originals.push((path, bytes));
        }
    }
    assert!(copy.join("scene.bin").is_file(), "fixture needs a snapshot");
    let session = ProjectSession::open(&copy).expect("open disposable fixture copy");
    let original_scene = scene_bytes(&session);
    let original_epoch = session.epoch();
    let original_pages = session.scene.read().pages.len();

    // Exercise current text-bearing history frames after any legacy migration.
    let mut page = Page::new("compatibility probe", 800, 600);
    let node_id = NodeId::new();
    page.nodes.insert(
        node_id,
        Node {
            id: node_id,
            transform: Transform::default(),
            visible: true,
            kind: NodeKind::Text(TextData {
                translation: Some("猫🙂한글".into()),
                style_ranges: vec![TextStyleRange {
                    start: 3,
                    end: 7,
                    style: TextRangeStyle {
                        bold: Some(true),
                        ..Default::default()
                    },
                }],
                writing_direction: Some(TextDirection::Vertical),
                ..Default::default()
            }),
        },
    );
    session
        .apply(Op::AddPage {
            page,
            at: original_pages,
        })
        .unwrap();
    let edited_scene = scene_bytes(&session);
    assert_eq!(session.epoch(), original_epoch + 1);
    assert!(session.undo().unwrap().is_some());
    assert_eq!(scene_bytes(&session), original_scene);
    assert!(session.redo().unwrap().is_some());
    assert_eq!(scene_bytes(&session), edited_scene);
    let edited_epoch = session.epoch();
    drop(session);

    // No compaction yet: this reopen must recover solely through history replay.
    let session = ProjectSession::open(&copy).expect("replay migrated log plus new frames");
    assert_eq!(session.epoch(), edited_epoch);
    assert_eq!(scene_bytes(&session), edited_scene);
    session.compact().unwrap();
    let snapshot = std::fs::read(copy.join("scene.bin")).unwrap();
    assert_eq!(&snapshot[..6], b"KSCN\x08\x00");
    let log = std::fs::read(copy.join("history.log")).unwrap();
    assert_eq!(log, b"KHLG\x03\x00");
    drop(session);

    let session = ProjectSession::open(&copy).expect("reopen compacted v8 snapshot");
    assert_eq!(session.epoch(), edited_epoch);
    assert_eq!(scene_bytes(&session), edited_scene);
    drop(session);
    for (path, bytes) in originals {
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "source changed: {}",
            path.display()
        );
    }
    println!(
        "PASS {}: {original_pages} pages, epoch {original_epoch} -> {edited_epoch}",
        source.display()
    );
}

#[test]
#[ignore = "requires explicit local project paths in KOHARU_COMPAT_PROJECTS"]
fn local_project_copies_survive_migration_replay_and_compaction() {
    let paths = std::env::var_os("KOHARU_COMPAT_PROJECTS").expect("set KOHARU_COMPAT_PROJECTS");
    let paths = std::env::split_paths(&paths).collect::<Vec<_>>();
    assert!(!paths.is_empty());
    for path in paths {
        check_copy(&path);
    }
}
