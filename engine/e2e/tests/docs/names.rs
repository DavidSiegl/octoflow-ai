//! The names the docs may give: those the Rust defines, the engine's and the desktop shell's, read by parsing it, and
//! gate.py's, read as text.

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

/// The names the Rust of RUST_SOURCES and gate.py define. Panics when a file cannot be read, or a .rs file parsed.
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
    gate_names(&mut names);
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

/// Whether attrs hold #[cfg(test)].
fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg") && matches!(&a.meta, syn::Meta::List(list) if list.tokens.to_string() == "test")
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

    // A doc comment's text is no string the code uses.
    fn visit_attribute(&mut self, a: &'ast syn::Attribute) {
        if !a.path().is_ident("doc") {
            visit::visit_attribute(self, a);
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

/// Add gate.py's names to names, read from its text, since it is Python: each def, class and assignment at the left
/// margin as an item; as Class.member, each def and assignment of a class's body and each `self.name =` in it; and
/// each one-line string literal. A name defined otherwise, such as in a for, a with or an import, is not read. Panics
/// when gate.py cannot be read.
fn gate_names(names: &mut Names) {
    let path = ROOT.join("gate.py");
    let source = fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let mut class: Option<String> = None;
    for line in source.lines() {
        let body = line.trim_start();
        let indent = line.len() - body.len();
        if indent == 0 && !body.is_empty() && !body.starts_with('#') && !body.starts_with(')') {
            class = None;
        }
        if let Some(name) = defined_name(body) {
            if indent == 0 {
                if body.starts_with("class ") {
                    class = Some(name.to_string());
                }
                names.items.insert(name.to_string());
            } else if let Some(owner) = class.as_ref().filter(|_| indent == 4) {
                names.members.insert(format!("{owner}.{name}"));
            }
        }
        if let Some(owner) = &class {
            for (i, _) in body.match_indices("self.") {
                let rest = &body[i + 5..];
                let len = rest.find(|c: char| !crate::markdown::word_char(c)).unwrap_or(rest.len());
                let after = rest[len..].trim_start();
                let assigned = after.starts_with('=') && !after.starts_with("==")
                    || after.starts_with(':')
                    || ["+=", "-=", "|=", "*="].iter().any(|op| after.starts_with(op));
                if len > 0 && assigned {
                    names.members.insert(format!("{owner}.{}", &rest[..len]));
                }
            }
        }
        names.strings.extend(one_line_strings(line));
    }
}

/// The name a line of Python defines at its start: `def name(`, `async def name(`, `class name`, `name =` or
/// `name: type`; None for any other line.
fn defined_name(body: &str) -> Option<&str> {
    let rest = ["def ", "async def ", "class "].iter().find_map(|k| body.strip_prefix(k));
    let named = rest.unwrap_or(body);
    let len = named.find(|c: char| !crate::markdown::word_char(c)).unwrap_or(named.len());
    if len == 0 || named.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let after = named[len..].trim_start();
    let defines = if rest.is_some() {
        true
    } else {
        after.starts_with('=') && !after.starts_with("==") || after.starts_with(':') && !after.starts_with(":=")
    };
    defines.then(|| &named[..len])
}

/// The texts of a line's string literals that open and close on it, quoted '...' or "...", backslash escapes left as
/// written; a triple-quoted string's text is not one.
fn one_line_strings(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '#' {
            break;
        }
        if c != '"' && c != '\'' {
            continue;
        }
        if line[i..].starts_with(&c.to_string().repeat(3)) {
            break;
        }
        let mut text = String::new();
        let mut closed = false;
        while let Some((_, d)) = chars.next() {
            if d == '\\' {
                if let Some((_, e)) = chars.next() {
                    text.push(d);
                    text.push(e);
                }
            } else if d == c {
                closed = true;
                break;
            } else {
                text.push(d);
            }
        }
        if closed {
            found.push(text);
        }
    }
    found
}
