//! Module resolution exposed to codemods: what a specifier resolves to, what
//! a module's export lands on, and which specifier reaches a file.

/// Which kind of specifier `specifier_for` should prefer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecifierStyle {
    /// `./a/b`, `../c`.
    Relative,
    /// A tsconfig `paths` alias (`@/components/button`, `~utils`).
    Alias,
}

/// Options for `specifier_for`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecifierOptions {
    /// Prefer this style when both kinds reach the file.
    pub prefer: Option<SpecifierStyle>,
    /// The specifier being replaced. Its style (relative or alias), its alias
    /// prefix and whether it names an extension (`./x.js`) are kept when a
    /// specifier like it reaches the file. Takes precedence over `prefer`.
    pub like: Option<String>,
}
