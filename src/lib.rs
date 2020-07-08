//! Utility to traverse the file-system and inline modules that are declared as references to
//! other Rust files.

use proc_macro2::Span;
use std::{
    borrow::Cow,
    error, fmt, io,
    path::{Path, PathBuf},
};
use syn::spanned::Spanned;
use syn::ItemMod;

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
/// failures that occur while inlining referenced modules, create an `InlinerBuilder` with
/// `track_errors(true)` set.
pub fn parse_and_inline_modules(src_file: &Path) -> syn::File {
    InlinerBuilder::default()
        .parse_and_inline_modules(src_file)
        .unwrap()
}

/// A builder that can configure how to inline modules.
///
/// After creating a builder, set configuration options using the methods
/// taking `&mut self`, then parse and inline one or more files using
/// `parse_and_inline_modules`.
#[derive(Debug)]
pub struct InlinerBuilder {
    root: bool,
    track_errors: bool,
}

impl Default for InlinerBuilder {
    fn default() -> Self {
        InlinerBuilder {
            root: true,
            track_errors: false,
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

    /// Configures whether unexpanded modules (due to missing files or invalid Rust source code)
    /// will lead to an `Err` return value or not.
    ///
    /// Default: `false`.
    pub fn track_errors(&mut self, track_errors: bool) -> &mut Self {
        self.track_errors = track_errors;
        self
    }

    /// Parse the source code in `src_file` and return a `syn::File` that has all modules
    /// recursively inlined.
    pub fn parse_and_inline_modules(&self, src_file: &Path) -> Result<syn::File, Error> {
        self.parse_internal(src_file, FsResolver::default())
    }

    fn parse_internal<R: FileResolver + Clone>(
        &self,
        src_file: &Path,
        resolver: R,
    ) -> Result<syn::File, Error> {
        let mut errors = if self.track_errors {
            Some(vec![])
        } else {
            None
        };
        let result =
            Visitor::<R>::with_resolver(src_file, self.root, errors.as_mut(), Cow::Owned(resolver))
                .visit()
                .map_err(Error::Root)?;
        match errors {
            Some(ref errors) if errors.is_empty() => Ok(result),
            None => Ok(result),
            Some(errors) => Err(Error::Inline(IncompleteInlining::new(result, errors))),
        }
    }
}

/// An error that was encountered while reading, parsing or inlining a module.
#[derive(Debug)]
pub enum Error {
    /// An error happened while reading or parsing the initial file.
    ///
    /// For example, the initial file wasn't found or wasn't a valid Rust file.
    Root(ErrorKind),

    /// The initial file was successfully loaded, but some errors happened while attempting to
    /// inline modules.
    Inline(IncompleteInlining),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Root(kind) => write!(f, "error for initial file: {}", kind),
            Error::Inline(errors) => errors.fmt(f),
        }
    }
}

impl error::Error for Error {}

impl Error {
    /// Returns true if the error happened while reading or parsing the initial file.
    pub fn is_initial(&self) -> bool {
        match self {
            Error::Root(_) => true,
            Error::Inline(_) => false,
        }
    }

    /// Returns true if the error happened while inlining modules.
    pub fn is_inline(&self) -> bool {
        match self {
            Error::Root(_) => false,
            Error::Inline(_) => true,
        }
    }
}

/// The kind of error that was encountered for a particular file.
#[derive(Debug)]
pub enum ErrorKind {
    /// An error happened while opening or reading the file.
    Io(io::Error),

    /// Errors happened while using `syn` to parse the file.
    Parse(syn::Error),
}

impl From<io::Error> for ErrorKind {
    fn from(err: io::Error) -> Self {
        ErrorKind::Io(err)
    }
}

impl From<syn::Error> for ErrorKind {
    fn from(err: syn::Error) -> Self {
        ErrorKind::Parse(err)
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ErrorKind::Io(err) => write!(f, "IO error: {}", err),
            ErrorKind::Parse(err) => write!(f, "parse error: {}", err),
        }
    }
}

/// An inlining that was partially - but not completely - successful. The root file was parsed
/// successfully, but 1 or more referenced modules failed to load.
pub struct IncompleteInlining {
    output: syn::File,
    errors: Vec<InlineError>,
}

impl IncompleteInlining {
    /// Create a new `IncompleteInlining` with the best-effort output and the list of errors
    /// encountered.
    ///
    /// # Panics
    /// This function will panic if `errors` is empty. An empty error list means the inlining
    /// is complete, and this struct should not be used.
    pub(crate) fn new(output: syn::File, errors: Vec<InlineError>) -> Self {
        if errors.is_empty() {
            panic!("Inlining must encounter 1 or more errors to be incomplete");
        }

        Self { output, errors }
    }

    /// The best-effort result of inlining.
    pub fn output(&self) -> &syn::File {
        &self.output
    }

    /// The errors that kept the inlining from completing. This is guaranteed not to be empty.
    pub fn errors(&self) -> &[InlineError] {
        &self.errors
    }

    /// Break an incomplete inlining into the best-effort parsed result and the errors encountered.
    ///
    /// # Usage
    ///
    /// ```rust,norun
    /// # use syn_inline_mod::{Error, IncompleteInlining};
    /// # #[allow(unused_vars)]
    /// # fn handle_error(error: Error) {
    /// if let Error::Inline(err) = error {
    ///     let (output, errors) = err.into_output_and_errors();
    /// }
    /// # }
    /// ```
    pub fn into_output_and_errors(self) -> (syn::File, Vec<InlineError>) {
        (self.output, self.errors)
    }
}

impl fmt::Debug for IncompleteInlining {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.errors.fmt(f)
    }
}

impl fmt::Display for IncompleteInlining {
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
    kind: ErrorKind,
}

impl InlineError {
    pub(crate) fn new(
        src_path: impl Into<PathBuf>,
        item_mod: &ItemMod,
        path: impl Into<PathBuf>,
        kind: ErrorKind,
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
    pub fn kind(&self) -> &ErrorKind {
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
            .parse_internal(Path::new("src/lib.rs"), make_test_env())
            .unwrap();

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

        let result = InlinerBuilder::default()
            .track_errors(true)
            .parse_internal(Path::new("src/lib.rs"), env);

        match result {
            Err(Error::Inline(incomplete)) => {
                let errors = &incomplete.errors;
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
                    ErrorKind::Io(err) => err,
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
                    ErrorKind::Parse(_) => {}
                    ErrorKind::Io(_) => panic!("expected ErrorKind::Parse, found {}", error.kind()),
                }
            }
            Ok(parsed) => panic!(
                "Expected to get errors in parse/inline: {}",
                parsed.into_token_stream()
            ),
            _ => unreachable!(),
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
            .parse_internal(Path::new("src/lib.rs"), env)
            .unwrap();

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
            .parse_internal(Path::new("src/lib.rs"), env)
            .unwrap();

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
