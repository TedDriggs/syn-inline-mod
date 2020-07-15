//! Utility to traverse the file-system and inline modules that are declared as references to
//! other Rust files.

use os_str_bytes::OsStringBytes;
use proc_macro2::Span;
use std::{
    error, fmt, io,
    path::{Path, PathBuf},
};
use syn::spanned::Spanned;
use syn::{Attribute, ItemMod, LitByteStr};

mod mod_path;
mod resolver;
mod visitor;

pub(crate) use mod_path::*;
pub(crate) use resolver::*;
pub(crate) use visitor::Visitor;

/// Parse the source code in `src_file` and return a `syn::File` that has all modules
/// recursively inlined.
///
/// This is equivalent to using an `InlinerBuilder` with the default settings.
///
/// # Panics
///
/// This function will panic if `src_file` cannot be opened or does not contain valid Rust
/// source code.
///
/// # Error Handling
///
/// This function ignores most error cases to return a best-effort result. To be informed of
/// failures that occur while inlining referenced modules, create an `InlinerBuilder` instead.
pub fn parse_and_inline_modules(src_file: &Path) -> syn::File {
    InlinerBuilder::default()
        .parse_and_inline_modules(src_file)
        .unwrap()
        .output
}

/// The string `syn_inline_mod_path`. This acts as a marker for inlined paths.
pub(crate) static SYN_INLINE_MOD_PATH: &str = "syn_inline_mod_path";

/// A module path in a list of attributes, found by `find_mod_path`.
#[derive(Clone, Debug)]
pub struct InlineModPath<'ast> {
    /// The path this module resides in. Uses the same base (relative or absolute) as the
    /// original path passed in.
    pub path: PathBuf,

    /// The outer attributes. These are of the form `#[attribute]` and are in the path that
    /// imported this module.
    pub outer_attributes: &'ast [Attribute],

    /// The inner attributes. These are of the form `#![attribute]` and are defined in `path`.
    ///
    /// The attributes listed in `inner_attributes` will retain their preceding `!` -- that is
    /// technically not valid Rust but is done this way for simplicity.
    pub inner_attributes: &'ast [Attribute],
}

/// Given a list of attributes from an inlined module, search for the first attribute of the form
/// `#[syn_inline_mod_path(b"...")]`. If it is found, then return a triple with:
/// * the module path
/// * the outer attributes
/// * the inner attributes
///
/// See the documentation for `InlinerBuilder::annotate_paths` for more information.
pub fn find_mod_path(attrs: &[Attribute]) -> Option<InlineModPath> {
    let mut path = None;
    let position = attrs.iter().position(|attr| {
        if attr.path.is_ident(SYN_INLINE_MOD_PATH) {
            // Ignore the attribute if it isn't of the correct form.
            if let Ok(lit) = attr.parse_args::<LitByteStr>() {
                let value = lit.value();
                if let Ok(extracted_path) = PathBuf::from_vec(value) {
                    // Record the extracted path in `path` above.
                    path = Some(extracted_path);
                }
                return true;
            }
        }
        false
    })?;
    Some(InlineModPath {
        path: path.expect("position() returning Some means a path was found"),
        outer_attributes: &attrs[0..position],
        inner_attributes: &attrs[position + 1..],
    })
}

/// A builder that can configure how to inline modules.
///
/// After creating a builder, set configuration options using the methods
/// taking `&mut self`, then parse and inline one or more files using
/// `parse_and_inline_modules`.
#[derive(Debug)]
pub struct InlinerBuilder {
    root: bool,
    annotate_paths: bool,
}

impl Default for InlinerBuilder {
    fn default() -> Self {
        InlinerBuilder {
            root: true,
            annotate_paths: false,
        }
    }
}

impl InlinerBuilder {
    /// Create a new `InlinerBuilder` with the default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures whether the module being parsed is a root module or not.
    ///
    /// A root module is one that is passed directly to `rustc`. A non-root
    /// module is one that is included from another module using a `mod` item.
    ///
    /// Default: `true`.
    pub fn root(&mut self, root: bool) -> &mut Self {
        self.root = root;
        self
    }

    /// Configures whether modules being inlined should be annotated with the paths they
    /// belong to.
    ///
    /// If this is true, every module that is inlined will have an annotation
    /// `#[syn_inline_mod_path("...")]` added to it. This annotation can then be used to
    /// figure out which path a particular module lives in.
    ///
    /// This is useful when mapping `Span` instances to the modules they come in.
    ///
    /// Default: `false`.
    ///
    /// ## Example
    ///
    /// If `foo_crate/src/lib.rs` contains:
    ///
    /// ```ignore
    /// #[outer_attr]
    /// pub mod foo_mod;
    /// ```
    ///
    /// and `foo_crate/src/foo_mod.rs` contains:
    ///
    /// ```ignore
    /// #![inner_attr]
    ///
    /// fn foo() {}
    /// ```
    ///
    /// then setting this option to `true` will result in a module that looks something like:
    ///
    /// ```ignore
    /// #[outer_attr]
    /// #[syn_inline_mod_path(b"foo_crate/src/foo_mod.rs")]
    /// #![inner_attr]
    /// pub mod foo_mod {
    ///     fn foo() {}
    /// }
    /// ```
    ///
    /// Note that paths are encoded as byte strings to allow non-Unicode paths to be represented.
    pub fn annotate_paths(&mut self, annotate: bool) -> &mut Self {
        self.annotate_paths = annotate;
        self
    }

    /// Parse the source code in `src_file` and return an `InliningResult` that has all modules
    /// recursively inlined.
    pub fn parse_and_inline_modules(&self, src_file: &Path) -> Result<InliningResult, Error> {
        self.parse_internal(src_file, &mut FsResolver::new(|_: &Path, _| {}))
    }

    /// Parse the source code in `src_file` and return an `InliningResult` that has all modules
    /// recursively inlined. Call the given callback whenever a file is loaded from disk (regardless
    /// of if it parsed successfully).
    pub fn inline_with_callback(
        &self,
        src_file: &Path,
        on_load: impl FnMut(&Path, String),
    ) -> Result<InliningResult, Error> {
        self.parse_internal(src_file, &mut FsResolver::new(on_load))
    }

    fn parse_internal<R: FileResolver>(
        &self,
        src_file: &Path,
        resolver: &mut R,
    ) -> Result<InliningResult, Error> {
        // XXX There is no way for library callers to disable error tracking,
        // but until we're sure that there's no performance impact of enabling it
        // we'll let downstream code think that error tracking is optional.
        let mut errors = Some(vec![]);
        let result = Visitor::<R>::new(
            src_file,
            self.root,
            self.annotate_paths,
            errors.as_mut(),
            resolver,
        )
        .visit()?;
        Ok(InliningResult::new(
            result,
            errors.unwrap_or_default(),
            self.annotate_paths,
        ))
    }
}

/// An error that was encountered while reading, parsing or inlining a module.
///
/// Errors block further progress on inlining, but do not invalidate other progress.
/// Therefore, only an error on the initially-passed-in-file is fatal to inlining.
#[derive(Debug)]
pub enum Error {
    /// An error happened while opening or reading the file.
    Io(io::Error),

    /// Errors happened while using `syn` to parse the file.
    Parse(syn::Error),
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            Error::Parse(err) => Some(err),
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

impl From<syn::Error> for Error {
    fn from(err: syn::Error) -> Self {
        Error::Parse(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(_) => write!(f, "IO error"),
            Error::Parse(_) => write!(f, "parse error"),
        }
    }
}

/// The result of a best-effort attempt at inlining.
///
/// This struct guarantees that the origin file was readable and valid Rust source code, but
/// `errors` must be inspected to check if everything was inlined successfully.
pub struct InliningResult {
    output: syn::File,
    errors: Vec<InlineError>,
    paths_annotated: bool,
}

impl InliningResult {
    /// Create a new `InliningResult` with the best-effort output and any errors encountered
    /// during the inlining process.
    pub(crate) fn new(output: syn::File, errors: Vec<InlineError>, paths_annotated: bool) -> Self {
        InliningResult {
            output,
            errors,
            paths_annotated,
        }
    }

    /// The best-effort result of inlining.
    pub fn output(&self) -> &syn::File {
        &self.output
    }

    /// The errors that kept the inlining from completing. May be empty if there were no errors.
    pub fn errors(&self) -> &[InlineError] {
        &self.errors
    }

    /// Whether the result has any errors. `false` implies that all inlining operations completed
    /// successfully.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Whether paths were annotated using `InlinerBuilder::annotate_paths`.
    ///
    /// If this is `true`, `find_mod_path` may be used to figure out the path a module is defined
    /// in.
    pub fn paths_annotated(&self) -> bool {
        self.paths_annotated
    }

    /// Break an incomplete inlining into the best-effort parsed result and the errors encountered.
    ///
    /// # Usage
    ///
    /// ```rust,ignore
    /// # #![allow(unused_variables)]
    /// # use std::path::Path;
    /// # use syn_inline_mod::InlinerBuilder;
    /// let result = InlinerBuilder::default().parse_and_inline_modules(Path::new("foo.rs"));
    /// match result {
    ///     Err(e) => unimplemented!(),
    ///     Ok(r) if r.has_errors() => {
    ///         let (best_effort, errors) = r.into_output_and_errors();
    ///         // do things with the partial output and the errors
    ///     },
    ///     Ok(r) => {
    ///         let (complete, _) = r.into_output_and_errors();
    ///         // do things with the completed output
    ///     }
    /// }
    /// ```
    pub fn into_output_and_errors(self) -> (syn::File, Vec<InlineError>) {
        (self.output, self.errors)
    }
}

impl fmt::Debug for InliningResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.errors.fmt(f)
    }
}

impl fmt::Display for InliningResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Inlining partially completed before errors:")?;
        for error in &self.errors {
            writeln!(f, " * {}", error)?;
        }

        Ok(())
    }
}

/// An error that happened while attempting to inline a module.
#[derive(Debug)]
pub struct InlineError {
    src_path: PathBuf,
    module_name: String,
    src_span: Span,
    path: PathBuf,
    kind: Error,
}

impl InlineError {
    pub(crate) fn new(
        src_path: impl Into<PathBuf>,
        item_mod: &ItemMod,
        path: impl Into<PathBuf>,
        kind: Error,
    ) -> Self {
        Self {
            src_path: src_path.into(),
            module_name: item_mod.ident.to_string(),
            src_span: item_mod.span(),
            path: path.into(),
            kind,
        }
    }

    /// Returns the source path where the error originated.
    ///
    /// The file at this path parsed correctly, but it caused the file at `self.path()` to be read.
    pub fn src_path(&self) -> &Path {
        &self.src_path
    }

    /// Returns the name of the module that was attempted to be inlined.
    pub fn module_name(&self) -> &str {
        &self.module_name
    }

    /// Returns the `Span` (including line and column information) in the source path that caused
    /// `self.path()` to be included.
    pub fn src_span(&self) -> proc_macro2::Span {
        self.src_span
    }

    /// Returns the path where the error happened.
    ///
    /// Reading and parsing this file failed for the reason listed in `self.kind()`.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the reason for this error happening.
    pub fn kind(&self) -> &Error {
        &self.kind
    }
}

impl fmt::Display for InlineError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let start = self.src_span.start();
        write!(
            f,
            "{}:{}:{}: error while including {}: {}",
            self.src_path.display(),
            start.line,
            start.column,
            self.path.display(),
            self.kind
        )
    }
}

#[cfg(test)]
mod tests {
    use quote::{quote, ToTokens};

    use super::*;

    fn make_test_env() -> TestResolver {
        let mut env = TestResolver::default();
        env.register("src/lib.rs", "mod first;");
        env.register("src/first/mod.rs", "mod second;");
        env.register(
            "src/first/second.rs",
            r#"
            #[doc = " Documentation"]
            mod third {
                mod fourth;
            }

            pub fn sample() -> usize { 4 }
            "#,
        );
        env.register(
            "src/first/second/third/fourth.rs",
            "pub fn another_fn() -> bool { true }",
        );
        env
    }

    /// Run a full test, exercising the entirety of the functionality in this crate.
    #[test]
    fn happy_path() {
        let result = InlinerBuilder::default()
            .parse_internal(Path::new("src/lib.rs"), &mut make_test_env())
            .unwrap()
            .output;

        assert_eq!(
            result.into_token_stream().to_string(),
            quote! {
                mod first {
                    mod second {
                        #[doc = " Documentation"]
                        mod third {
                            mod fourth {
                                pub fn another_fn() -> bool {
                                    true
                                }
                            }
                        }

                        pub fn sample() -> usize {
                            4
                        }
                    }
                }
            }
            .to_string()
        );
    }

    /// Test case involving missing and invalid modules
    #[test]
    fn missing_module() {
        let mut env = TestResolver::default();
        env.register("src/lib.rs", "mod missing;\nmod invalid;");
        env.register("src/invalid.rs", "this-is-not-valid-rust!");

        let result = InlinerBuilder::default().parse_internal(Path::new("src/lib.rs"), &mut env);

        if let Ok(r) = result {
            let errors = &r.errors;
            assert_eq!(errors.len(), 2, "expected 2 errors");

            let error = &errors[0];
            assert_eq!(
                error.src_path(),
                Path::new("src/lib.rs"),
                "correct source path"
            );
            assert_eq!(error.module_name(), "missing");
            assert_eq!(error.src_span().start().line, 1);
            assert_eq!(error.src_span().start().column, 0);
            assert_eq!(error.src_span().end().line, 1);
            assert_eq!(error.src_span().end().column, 12);
            assert_eq!(error.path(), Path::new("src/missing/mod.rs"));
            let io_err = match error.kind() {
                Error::Io(err) => err,
                _ => panic!("expected ErrorKind::Io, found {}", error.kind()),
            };
            assert_eq!(io_err.kind(), io::ErrorKind::NotFound);

            let error = &errors[1];
            assert_eq!(
                error.src_path(),
                Path::new("src/lib.rs"),
                "correct source path"
            );
            assert_eq!(error.module_name(), "invalid");
            assert_eq!(error.src_span().start().line, 2);
            assert_eq!(error.src_span().start().column, 0);
            assert_eq!(error.src_span().end().line, 2);
            assert_eq!(error.src_span().end().column, 12);
            assert_eq!(error.path(), Path::new("src/invalid.rs"));
            match error.kind() {
                Error::Parse(_) => {}
                Error::Io(_) => panic!("expected ErrorKind::Parse, found {}", error.kind()),
            }
        } else {
            unreachable!();
        }
    }

    /// Test case involving `cfg_attr` from the original request for implementation.
    ///
    /// Right now, this test fails for two reasons:
    ///
    /// 1. We don't look for `cfg_attr` elements
    /// 2. We don't have a way to insert new items
    ///
    /// The first fix is simpler, but the second one would be difficult.
    #[test]
    #[should_panic]
    fn cfg_attrs() {
        let mut env = TestResolver::default();
        env.register(
            "src/lib.rs",
            r#"
            #[cfg(feature = "m1")]
            mod m1;

            #[cfg_attr(feature = "m2", path = "m2.rs")]
            #[cfg_attr(not(feature = "m2"), path = "empty.rs")]
            mod placeholder;
        "#,
        );
        env.register("src/m1.rs", "struct M1;");
        env.register(
            "src/m2.rs",
            "
        //! module level doc comment

        struct M2;
        ",
        );
        env.register("src/empty.rs", "");

        let result = InlinerBuilder::default()
            .parse_internal(Path::new("src/lib.rs"), &mut env)
            .unwrap()
            .output;

        assert_eq!(
            result.into_token_stream().to_string(),
            quote! {
                #[cfg(feature = "m1")]
                mod m1 {
                    struct M1;
                }

                #[cfg(feature = "m2")]
                mod placeholder {
                    //! module level doc comment

                    struct M2;
                }

                #[cfg(not(feature = "m2"))]
                mod placeholder {

                }
            }
            .to_string()
        )
    }

    #[test]
    fn cfg_attrs_revised() {
        let mut env = TestResolver::default();
        env.register(
            "src/lib.rs",
            r#"
            #[cfg(feature = "m1")]
            mod m1;

            #[cfg(feature = "m2")]
            #[path = "m2.rs"]
            mod placeholder;

            #[cfg(not(feature = "m2"))]
            #[path = "empty.rs"]
            mod placeholder;
        "#,
        );
        env.register("src/m1.rs", "struct M1;");
        env.register(
            "src/m2.rs",
            r#"
            #![doc = " module level doc comment"]

            struct M2;
            "#,
        );
        env.register("src/empty.rs", "");

        let result = InlinerBuilder::default()
            .parse_internal(Path::new("src/lib.rs"), &mut env)
            .unwrap()
            .output;

        assert_eq!(
            result.into_token_stream().to_string(),
            quote! {
                #[cfg(feature = "m1")]
                mod m1 {
                    struct M1;
                }

                #[cfg(feature = "m2")]
                #[path = "m2.rs"]
                mod placeholder {
                    #![doc = " module level doc comment"]

                    struct M2;
                }

                #[cfg(not(feature = "m2"))]
                #[path = "empty.rs"]
                mod placeholder {

                }
            }
            .to_string()
        )
    }
}
