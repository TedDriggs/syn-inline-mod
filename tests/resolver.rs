//! Test that syn-inline-mod can resolve this crate's lib.rs properly.

use std::path::{Path, PathBuf};
use syn::Item;
use syn_inline_mod::{find_mod_path, InlineModPath, InlinerBuilder};

#[test]
fn resolve_lib() {
    let builder = InlinerBuilder::new();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_rs = manifest_dir.join("src/lib.rs");

    let (_, files_seen) = inline(&builder, &lib_rs);

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

#[test]
fn resolve_example_fixture() {
    let mut builder = InlinerBuilder::new();
    builder.annotate_paths(true);

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example_lib_rs = manifest_dir.join("fixtures/example/lib.rs");

    let (inlined, _) = inline(&builder, &example_lib_rs);

    assert_eq!(inlined.items.len(), 1, "inlined lib.rs has 1 item");
    let item = &inlined.items[0];
    let foo_mod = match item {
        Item::Mod(foo_mod) => foo_mod,
        _ => panic!("expected Item::Mod, found {:?}", item),
    };
    assert_eq!(foo_mod.ident, "foo", "correct ident name for foo");

    let InlineModPath {
        path,
        outer_attributes,
        inner_attributes,
    } = find_mod_path(&foo_mod.attrs).expect("foo should be annotated with path");
    let rel_path = path
        .strip_prefix(manifest_dir)
        .expect("path should be relative to manifest dir");
    assert_eq!(
        rel_path.to_str(),
        Some("fixtures/example/foo.rs"),
        "correct annotated path"
    );
    assert_eq!(outer_attributes.len(), 1, "correct outer attribute length");
    assert!(
        outer_attributes[0].path.is_ident("outer_attr"),
        "correct outer attribute"
    );
    assert_eq!(inner_attributes.len(), 1, "correct inner attribute length");
    assert!(
        inner_attributes[0].path.is_ident("inner_attr"),
        "correct inner attribute"
    );

    // Check for foo -> bar mapping.
    let (_, items) = foo_mod
        .content
        .as_ref()
        .expect("inlined module has content");
    assert_eq!(items.len(), 1, "foo has correct number of items");
    let bar_mod = match &items[0] {
        Item::Mod(bar_mod) => bar_mod,
        _ => panic!("expected Item::Mod, found {:?}", item),
    };
    let InlineModPath { path, .. } =
        find_mod_path(&bar_mod.attrs).expect("bar should be annotated with path");
    let rel_path = path
        .strip_prefix(manifest_dir)
        .expect("path should be relative to manifest dir");
    assert_eq!(
        rel_path.to_str(),
        Some("fixtures/example/foo/bar.rs"),
        "correct annotated path"
    );
}

/// Inlines a file and returns the inlined struct, the list of files seen and their contents.
fn inline(builder: &InlinerBuilder, path: &Path) -> (syn::File, Vec<(PathBuf, String)>) {
    let mut files_seen = vec![];

    let res = builder
        .inline_with_callback(&path, |path, file| {
            files_seen.push((path.to_path_buf(), file));
        })
        .unwrap_or_else(|err| {
            panic!(
                "{} should parse successfully, but failed with {}",
                path.display(),
                err
            )
        });
    assert!(!res.has_errors(), "result has no errors");
    (res.into_output_and_errors().0, files_seen)
}
