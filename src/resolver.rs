use crate::Error;
use std::path::Path;

/// A resolver that can turn paths into `syn::File` instances.
pub(crate) trait FileResolver {
    /// Check if `path` exists in the backing data store.
    fn path_exists(&self, path: &Path) -> bool;

    /// Resolves the given path into a file.
    ///
    /// Returns an error if the file couldn't be loaded or parsed as valid Rust.
    fn resolve(&mut self, path: &Path) -> Result<syn::File, Error>;
}

#[derive(Clone)]
pub(crate) struct FsResolver<F> {
    on_load: F,
}

impl<F> FsResolver<F> {
    pub(crate) fn new(on_load: F) -> Self {
        Self { on_load }
    }
}

impl<F> FileResolver for FsResolver<F>
where
    F: FnMut(&Path, String),
{
    fn path_exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn resolve(&mut self, path: &Path) -> Result<syn::File, Error> {
        let src = std::fs::read_to_string(path)?;
        let res = syn::parse_file(&src);
        // Call the callback whether the file parsed successfully or not.
        (self.on_load)(path, src);
        Ok(res?)
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
        self.files
            .insert(Path::new(path).to_path_buf(), contents.into());
    }
}

#[cfg(test)]
impl FileResolver for TestResolver {
    fn path_exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn resolve(&mut self, path: &Path) -> Result<syn::File, Error> {
        let src = self.files.get(path).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "path not in test resolver hashmap",
            )
        })?;
        Ok(syn::parse_file(src)?)
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

    fn resolve(&mut self, path: &Path) -> Result<syn::File, Error> {
        Ok(syn::parse_file(&format!(
            r#"const PATH: &str = "{}";"#,
            path.to_str().unwrap()
        ))?)
    }
}
