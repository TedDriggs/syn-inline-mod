//! Utility to traverse the file-system and inline modules that are declared as references to
//! other Rust files.

mod mod_path;
mod resolver;
mod visitor;

pub(crate) use mod_path::*;
pub(crate) use resolver::*;
pub(crate) use visitor::Visitor;

/// Parse the source code in `src_file` and return a `syn::File` that has all modules
/// recursively inlined.
pub fn parse_and_inline_modules(src_file: &std::path::Path) -> syn::File {
    Visitor::<FsResolver>::new(src_file).visit()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::Path;

    use quote::{quote, ToTokens};

    use crate::{TestResolver, Visitor};

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
        let mut visitor = Visitor::<TestResolver>::with_resolver(
            &Path::new("src/lib.rs"),
            Cow::Owned(make_test_env()),
        );

        assert_eq!(
            visitor.visit().into_token_stream().to_string(),
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
}
