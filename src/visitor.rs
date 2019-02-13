use std::borrow::Cow;
use std::path::Path;

use syn::visit_mut::VisitMut;
use syn::ItemMod;

use crate::{FileResolver, FsResolver, ModContext};

pub(crate) struct Visitor<'a, R: Clone> {
    /// The current file's path.
    path: &'a Path,
    /// The stack of `mod` entries where the visitor is currently located. This is needed
    /// for cases where modules are declared inside inline modules.
    mod_context: ModContext,
    /// The resolver that can be used to turn paths into `syn::File` instances. This removes
    /// a direct file-system dependency so the expander can be tested.
    resolver: Cow<'a, R>,
}

impl<'a, R: FileResolver + Default + Clone> Visitor<'a, R> {
    /// Create a new visitor with a default instance of the specified `FileResolver` type.
    pub fn new(path: &'a Path) -> Self {
        Self::with_resolver(path, Cow::Owned(R::default()))
    }
}

impl<'a, R: FileResolver + Clone> Visitor<'a, R> {
    /// Create a new visitor with the specified `FileResolver` instance. This will be
    /// used by all spawned visitors as we recurse down through the source code.
    pub fn with_resolver(path: &'a Path, resolver: Cow<'a, R>) -> Self {
        Self {
            path,
            resolver,
            mod_context: Default::default(),
        }
    }

    pub fn visit(&mut self) -> syn::File {
        let mut syntax = self.resolver.resolve(self.path);
        self.visit_file_mut(&mut syntax);
        syntax
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
            let file = self
                .mod_context
                .relative_to(self.path)
                .into_iter()
                .find(|p| self.resolver.path_exists(&p))
                .map(|path| Visitor::with_resolver(&path, self.resolver.clone()).visit());

            if let Some(syn::File { attrs, items, .. }) = file {
                i.attrs.extend(attrs);
                i.content = Some((Default::default(), items));
            }
        }

        self.mod_context.pop();
    }
}

impl<'a> From<&'a Path> for Visitor<'a, FsResolver> {
    fn from(path: &'a Path) -> Self {
        Visitor::<FsResolver>::new(path)
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
        let mut visitor = Visitor::<PathCommentResolver>::new(&path);
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
        let mut visitor = Visitor::<PathCommentResolver>::new(&path);
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
