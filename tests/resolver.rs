//! Test that syn-inline-mod can resolve this crate's lib.rs properly.

use std::path::Path;
use syn_inline_mod::InlinerBuilder;

#[test]
fn resolve_lib() {
    let builder = InlinerBuilder::new();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_rs = manifest_dir.join("src/lib.rs");

    let mut files_seen = vec![];

    let res = builder
        .inline_with_callback(&lib_rs, |path, file| {
            files_seen.push((path.to_path_buf(), file));
        })
        .expect("src/lib.rs should parse successfully");
    assert!(!res.has_errors(), "result has no errors");

    // Ensure that the list of files is correct.
    let file_list: Vec<_> = files_seen
        .iter()
        .map(|(path, _)| {
            let rel_path = path
                .strip_prefix(manifest_dir)
                .expect("path should be relative to manifest dir");
            rel_path.to_str().expect("path is valid Unicode")
        })
        .collect();

    // The order visited should be the same as the order in which "mod" statements are listed.
    assert_eq!(
        file_list,
        vec![
            "src/lib.rs",
            "src/mod_path.rs",
            "src/resolver.rs",
            "src/visitor.rs",
        ]
    );

    for (path, contents) in &files_seen {
        let disk_contents = std::fs::read_to_string(path).expect("reading contents failed");
        assert_eq!(&disk_contents, contents, "file contents match");
    }
}
