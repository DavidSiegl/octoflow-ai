//! The names the docs may give: those the Rust defines, the engine's and the desktop shell's, read by parsing it.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use syn::visit::{self, Visit};

use crate::ROOT;

/// The names a program defines, as the docs give them.
#[derive(Default)]
pub struct Names {
    /// Each item's name: fn (also a method and an associated one), struct, enum, trait, type alias, const, static and
    /// mod.
    pub items: HashSet<String>,
    /// Owner.member: each fn, const and type of an impl of a type or of a trait, as `Workflow.run`, and each named
    /// field of a struct.
    pub members: HashSet<String>,
    /// The text of each string literal outside the tests and fakes, which is how the code names an environment
    /// variable or a Jenkins parameter.
    pub strings: HashSet<String>,
}

/// The directories of the Rust whose names the docs give, under the repository's root: the engine's and the desktop
/// shell's.
const RUST_SOURCES: [&str; 2] = ["engine/src", "ui/src-tauri/src"];

/// The names the Rust of RUST_SOURCES defines. Panics when a file cannot be read, or a .rs file parsed.
pub fn defined() -> Names {
    let mut names = Names::default();
    for src in RUST_SOURCES.map(|dir| ROOT.join(dir)) {
        for path in rust_files(&src) {
            let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
            let file = syn::parse_file(&source).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()));
            let relative = path.strip_prefix(&src).unwrap_or(&path);
            let mut reader = Reader { names: &mut names, in_tests: is_test_file(relative) };
            reader.visit_file(&file);
        }
    }
    names
}

/// The .rs files under dir, in no particular order; panics when a directory cannot be listed.
fn rust_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()));
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.unwrap_or_else(|e| panic!("listing {}: {e}", dir.display())).path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files
}

/// Whether the file at path, relative to its source directory, holds only tests or fakes: a tests.rs or a file under
/// a tests/, and the fakes and fixtures the modules take in under #[cfg(test)].
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    ["tests.rs", "fake.rs", "fixture.rs", "test_support.rs"].contains(&name)
        || path.components().any(|c| c.as_os_str() == "tests")
}

/// Whether attrs hold a cfg that only a test build meets: #[cfg(test)] or #[cfg(all(test, ...))].
fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let syn::Meta::List(list) = &a.meta else { return false };
        let predicate = list.tokens.to_string().replace(' ', "");
        let all = predicate.strip_prefix("all(").and_then(|p| p.strip_suffix(')'));
        a.path().is_ident("cfg") && (predicate == "test" || all.is_some_and(|p| p.split(',').any(|c| c == "test")))
    })
}

/// The last name of a type's path, as `Workflow` of `crate::workflow::Workflow<H>`; None for a type that is no path,
/// such as a reference or a tuple.
fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// A visitor that adds the names of what it visits to names.
struct Reader<'a> {
    names: &'a mut Names,
    /// true means the code visited is a test's or a fake's, whose strings name nothing of the engine's.
    in_tests: bool,
}

impl Reader<'_> {
    fn item(&mut self, ident: &syn::Ident) {
        self.names.items.insert(ident.to_string());
    }

    fn member(&mut self, owner: &str, ident: &syn::Ident) {
        self.names.members.insert(format!("{owner}.{ident}"));
    }

    /// Visit an item under attrs: as test code when they hold #[cfg(test)].
    fn under(&mut self, attrs: &[syn::Attribute], visit: impl FnOnce(&mut Self)) {
        let outer = self.in_tests;
        self.in_tests |= cfg_test(attrs);
        visit(self);
        self.in_tests = outer;
    }
}

impl<'ast> Visit<'ast> for Reader<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs: &[syn::Attribute] = match item {
            syn::Item::Fn(i) => &i.attrs,
            syn::Item::Mod(i) => &i.attrs,
            syn::Item::Impl(i) => &i.attrs,
            syn::Item::Const(i) => &i.attrs,
            syn::Item::Static(i) => &i.attrs,
            syn::Item::Struct(i) => &i.attrs,
            syn::Item::Enum(i) => &i.attrs,
            syn::Item::Trait(i) => &i.attrs,
            syn::Item::Use(i) => &i.attrs,
            syn::Item::Type(i) => &i.attrs,
            syn::Item::Union(i) => &i.attrs,
            syn::Item::Macro(i) => &i.attrs,
            syn::Item::TraitAlias(i) => &i.attrs,
            syn::Item::ExternCrate(i) => &i.attrs,
            syn::Item::ForeignMod(i) => &i.attrs,
            _ => &[],
        };
        self.under(attrs, |r| visit::visit_item(r, item));
    }

    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.item(&i.sig.ident);
        visit::visit_item_fn(self, i);
    }

    fn visit_item_struct(&mut self, i: &'ast syn::ItemStruct) {
        self.item(&i.ident);
        let owner = i.ident.to_string();
        for field in &i.fields {
            if let Some(name) = &field.ident {
                self.member(&owner, name);
            }
        }
        visit::visit_item_struct(self, i);
    }

    fn visit_item_enum(&mut self, i: &'ast syn::ItemEnum) {
        self.item(&i.ident);
        visit::visit_item_enum(self, i);
    }

    fn visit_item_type(&mut self, i: &'ast syn::ItemType) {
        self.item(&i.ident);
        visit::visit_item_type(self, i);
    }

    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.item(&i.ident);
        visit::visit_item_const(self, i);
    }

    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.item(&i.ident);
        visit::visit_item_static(self, i);
    }

    fn visit_item_mod(&mut self, i: &'ast syn::ItemMod) {
        self.item(&i.ident);
        visit::visit_item_mod(self, i);
    }

    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.item(&i.ident);
        let owner = i.ident.to_string();
        for member in &i.items {
            let ident = match member {
                syn::TraitItem::Fn(f) => &f.sig.ident,
                syn::TraitItem::Const(c) => &c.ident,
                syn::TraitItem::Type(t) => &t.ident,
                _ => continue,
            };
            self.item(ident);
            self.member(&owner, ident);
        }
        visit::visit_item_trait(self, i);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let owner = type_name(&i.self_ty);
        for member in &i.items {
            let ident = match member {
                syn::ImplItem::Fn(f) => &f.sig.ident,
                syn::ImplItem::Const(c) => &c.ident,
                syn::ImplItem::Type(t) => &t.ident,
                _ => continue,
            };
            self.item(ident);
            if let Some(owner) = &owner {
                self.member(owner, ident);
            }
        }
        visit::visit_item_impl(self, i);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.under(&f.attrs, |r| visit::visit_impl_item_fn(r, f));
    }

    // A doc comment's text is no string the code uses. The arguments of a list attribute, as #[serde(rename = "x")],
    // are tokens to syn, read as expressions, as a macro's are.
    fn visit_attribute(&mut self, a: &'ast syn::Attribute) {
        use syn::punctuated::Punctuated;
        match &a.meta {
            syn::Meta::NameValue(_) if a.path().is_ident("doc") => {}
            syn::Meta::List(list) => {
                if let Ok(args) = list.parse_args_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated) {
                    for arg in &args {
                        self.visit_expr(arg);
                    }
                }
            }
            _ => visit::visit_attribute(self, a),
        }
    }

    // A macro's arguments are tokens to syn; those of the format! and assert! kind, comma-separated expressions, are
    // read as such, and the strings of any other, such as json!, go unread.
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        use syn::punctuated::Punctuated;
        if let Ok(args) = m.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }

    fn visit_lit_str(&mut self, s: &'ast syn::LitStr) {
        if !self.in_tests {
            self.names.strings.insert(s.value());
        }
    }
}
