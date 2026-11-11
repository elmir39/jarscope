mod common;

use std::io::Read;

use jarscope::manifest::parse_manifest;
use zip::{CompressionMethod, ZipArchive};

#[test]
fn helper_builds_readable_jar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let jar = common::make_jar_with_manifest(
        dir.path(),
        "t.jar",
        &[("Main-Class", "com.example.Main")],
        &[
            ("com/", vec![]),
            ("com/A.class", common::class_bytes(0, 52)),
            ("res.txt", b"hello".to_vec()),
        ],
    );
    let mut ar = ZipArchive::new(std::fs::File::open(jar).expect("open")).expect("zip");
    let names: Vec<String> = (0..ar.len())
        .map(|i| {
            ar.by_index(i)
                .expect("entry")
                .name()
                .expect("name")
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        ["META-INF/MANIFEST.MF", "com/", "com/A.class", "res.txt"]
    );

    let mut class = ar.by_name("com/A.class").expect("class");
    assert_eq!(class.compression(), CompressionMethod::Deflated);
    let mut head = Vec::new();
    (&mut class).take(8).read_to_end(&mut head).expect("read");
    assert_eq!(head, [0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 0x34]);
    drop(class);

    let mut m = Vec::new();
    ar.by_name("META-INF/MANIFEST.MF")
        .expect("mf")
        .read_to_end(&mut m)
        .expect("read");
    assert_eq!(
        parse_manifest(&m).get("Main-Class"),
        Some("com.example.Main")
    );
}
