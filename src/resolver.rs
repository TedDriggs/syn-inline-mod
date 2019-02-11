use std::fs::File;
use std::io::Read;
use std::path::Path;

/// A resolver that can turn paths into `syn::File` instances.
pub(crate) trait FileResolver {
    /// Check if `path` exists in the backing data store.
    fn path_exists(&self, path: &Path) -> bool;

    fn resolve(&self, path: &Path) -> syn::File;
}

#[derive(Default, Clone)]
pub(crate) struct FsResolver;

impl FileResolver for FsResolver {
    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn resolve(&self, path: &Path) -> syn::File {
        let mut file = File::open(&path).expect("Unable to open file");

        let mut src = String::new();
        file.read_to_string(&mut src).expect("Unable to read file");

        syn::parse_file(&src).expect("Unable to parse file")
    }
}

/// An alternate resolver which uses a static map of file contents for test purposes.
#[cfg(test)]
#[derive(Default, Clone)]
pub(crate) struct TestResolver {
    files: std::collections::HashMap<std::path::PathBuf, String>,
}

#[cfg(test)]
impl TestResolver {
    pub fn register(&mut self, path: &'static str, contents: &'static str) {
        self.files.insert(Path::new(path).to_path_buf(), contents.into());
    }
}

#[cfg(test)]
impl FileResolver for TestResolver {
    fn path_exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn resolve(&self, path: &Path) -> syn::File {
        let src = self
            .files
            .get(path)
            .expect("Test should only refer to files in context");
        syn::parse_file(src).expect("Test data should be parseable")
    }
}

/// A test resolver that emits a single-line comment containing the requested path
#[cfg(test)]
#[derive(Default, Clone)]
pub(crate) struct PathCommentResolver;

#[cfg(test)]
impl FileResolver for PathCommentResolver {
    fn path_exists(&self, _path: &Path) -> bool {
        true
    }

    fn resolve(&self, path: &Path) -> syn::File {
        syn::parse_file(&format!(r#"const PATH: &str = "{}";"#, path.to_str().unwrap())).unwrap()
    }
}