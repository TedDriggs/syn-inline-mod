//! Utility to traverse the file-system and inline modules that are declared as references to
//! other Rust files.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

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
pub fn parse_and_inline_modules(src_file: &std::path::Path) -> syn::File {
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
    error_not_found: bool,
}

impl Default for InlinerBuilder {
    fn default() -> Self {
        InlinerBuilder {
            root: true,
            error_not_found: false,
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

    /// Configures whether unexpanded modules (due to a missing file) will lead
    /// to an `Err` return value or not.
    ///
    /// Default: `false`.
    pub fn error_not_found(&mut self, error_not_found: bool) -> &mut Self {
        self.error_not_found = error_not_found;
        self
    }

    /// Parse the source code in `src_file` and return a `syn::File` that has all modules
    /// recursively inlined.
    pub fn parse_and_inline_modules(&self, src_file: &std::path::Path) -> Result<syn::File, Error> {
        self.parse_internal(src_file, FsResolver::default())
    }

    fn parse_internal<R: FileResolver + Clone>(
        &self,
        src_file: &std::path::Path,
        resolver: R,
    ) -> Result<syn::File, Error> {
        let mut errors = if self.error_not_found {
            Some(vec![])
        } else {
            None
        };
        let result =
            Visitor::<R>::with_resolver(src_file, self.root, errors.as_mut(), Cow::Owned(resolver))
                .visit();
        match errors {
            Some(ref errors) if errors.is_empty() => Ok(result),
            None => Ok(result),
            Some(errors) => Err(Error::NotFound(errors)),
        }
    }
}

/// A source location
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    _private: (),
}

impl SourceLocation {
    pub(crate) fn new(path: &Path, span: proc_macro2::Span) -> Self {
        SourceLocation {
            path: path.into(),
            line: span.start().line,
            column: span.start().column,
            _private: (),
        }
    }
}

/// An error that was encountered while inlining modules
#[derive(Debug)]
pub enum Error {
    /// The contents for one or more modules could not be found
    NotFound(Vec<(String, SourceLocation)>),
    #[doc(hidden)]
    __NonExhaustive,
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

    /// Test case involving a missing module
    #[test]
    fn missing_module() {
        let mut env = TestResolver::default();
        env.register("src/lib.rs", "mod missing;");

        let result = InlinerBuilder::default()
            .error_not_found(true)
            .parse_internal(Path::new("src/lib.rs"), env);

        match result {
            Err(Error::NotFound(errors)) => {
                assert_eq!(
                    errors,
                    [(
                        "missing".into(),
                        SourceLocation {
                            path: PathBuf::from("src/lib.rs"),
                            line: 1,
                            column: 0,
                            _private: ()
                        }
                    )]
                );
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
