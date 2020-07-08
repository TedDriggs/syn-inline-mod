use std::borrow::Cow;
use std::path::Path;

use syn::visit_mut::VisitMut;
use syn::ItemMod;

use crate::{Error, FileResolver, FsResolver, InlineError, ModContext};

pub(crate) struct Visitor<'a, R: Clone> {
    /// The current file's path.
    path: &'a Path,
    /// Whether this is the root file or not
    root: bool,
    /// The stack of `mod` entries where the visitor is currently located. This is needed
    /// for cases where modules are declared inside inline modules.
    mod_context: ModContext,
    /// The resolver that can be used to turn paths into `syn::File` instances. This removes
    /// a direct file-system dependency so the expander can be tested.
    resolver: Cow<'a, R>,
    /// A log of module items that weren't expanded.
    error_log: Option<&'a mut Vec<InlineError>>,
}

impl<'a, R: FileResolver + Default + Clone> Visitor<'a, R> {
    /// Create a new visitor with a default instance of the specified `FileResolver` type.
    fn new(path: &'a Path, root: bool, error_log: Option<&'a mut Vec<InlineError>>) -> Self {
        Self::with_resolver(path, root, error_log, Cow::Owned(R::default()))
    }
}

impl<'a, R: FileResolver + Clone> Visitor<'a, R> {
    /// Create a new visitor with the specified `FileResolver` instance. This will be
    /// used by all spawned visitors as we recurse down through the source code.
    pub fn with_resolver(
        path: &'a Path,
        root: bool,
        error_log: Option<&'a mut Vec<InlineError>>,
        resolver: Cow<'a, R>,
    ) -> Self {
        Self {
            path,
            root,
            resolver,
            error_log,
            mod_context: Default::default(),
        }
    }

    pub fn visit(&mut self) -> Result<syn::File, Error> {
        let mut syntax = self.resolver.resolve(self.path)?;
        self.visit_file_mut(&mut syntax);
        Ok(syntax)
    }
}

impl<'a, R: FileResolver + Clone> VisitMut for Visitor<'a, R> {
    fn visit_item_mod_mut(&mut self, i: &mut ItemMod) {
        self.mod_context.push(i.into());

        if let Some((_, items)) = &mut i.content {
            for item in items {
                self.visit_item_mut(item);
            }
        } else {
            // If we find a path that points to a satisfactory file, expand it
            // and replace the items with the file items. If something goes wrong,
            // leave the file alone.

            // candidates is guaranteed to be non-empty by ModContext::relative_to.
            let candidates = self.mod_context.relative_to(self.path, self.root);

            // Look for the first candidate file that exists.
            let first_candidate = candidates
                .iter()
                .find(|p| self.resolver.path_exists(p))
                .unwrap_or_else(|| {
                    // If no candidate exists, use the last file (which will error out while
                    // loading).
                    candidates
                        .iter()
                        .last()
                        .expect("candidates should be non-empty")
                });

            let mut visitor = Visitor::with_resolver(
                &first_candidate,
                false,
                self.error_log.as_mut().map(|v| &mut **v),
                self.resolver.clone(),
            );

            match visitor.visit() {
                Ok(syn::File { attrs, items, .. }) => {
                    i.attrs.extend(attrs);
                    i.content = Some((Default::default(), items));
                }
                Err(kind) => {
                    if let Some(ref mut errors) = self.error_log {
                        errors.push(InlineError::new(self.path, i, first_candidate, kind));
                    }
                }
            }
        }

        self.mod_context.pop();
    }
}

impl<'a> From<&'a Path> for Visitor<'a, FsResolver> {
    fn from(path: &'a Path) -> Self {
        Visitor::<FsResolver>::new(path, true, None)
    }
}

#[cfg(test)]
mod tests {
    use quote::{quote, ToTokens};
    use std::path::Path;
    use syn::visit_mut::VisitMut;

    use super::Visitor;
    use crate::PathCommentResolver;

    #[test]
    fn ident_in_lib() {
        let path = Path::new("./lib.rs");
        let mut visitor = Visitor::<PathCommentResolver>::new(&path, true, None);
        let mut file = syn::parse_file("mod c;").unwrap();
        visitor.visit_file_mut(&mut file);
        assert_eq!(
            file.into_token_stream().to_string(),
            quote! {
                mod c {
                    const PATH: &str = "./c.rs";
                }
            }
            .to_string()
        );
    }

    #[test]
    fn path_attr() {
        let path = std::path::Path::new("./lib.rs");
        let mut visitor = Visitor::<PathCommentResolver>::new(&path, true, None);
        let mut file = syn::parse_file(r#"#[path = "foo/bar.rs"] mod c;"#).unwrap();
        visitor.visit_file_mut(&mut file);
        assert_eq!(
            file.into_token_stream().to_string(),
            quote! {
                #[path = "foo/bar.rs"]
                mod c {
                    const PATH: &str = "./foo/bar.rs";
                }
            }
            .to_string()
        );
    }
}
