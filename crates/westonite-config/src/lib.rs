//! `westonite-config`: the re-specified configuration interface
//! (plan §5, decisions D9–D12).
//!
//! One serde `Config` model (TOML, kebab-case keys, unknown keys are
//! startup errors), a clap CLI, and generic `-o key.path=value` dotted
//! overrides patched into the TOML tree *before* deserialization.
//! Resolution order: defaults → file → `-o` overrides → flags, into an
//! immutable [`Settings`] resolved once at startup.  Consumers receive
//! typed slices; nobody re-reads the file or the CLI later.
//!
//! Completeness contract (risk R-G): every `[section]` key and CLI
//! option in `docs/frontend-capabilities.md` and
//! `docs/desktop-shell-capabilities.md` maps to a field below.  Values
//! that are really weston/libweston string grammars (modelines, XKB
//! names, gbm formats, transforms, ICC paths) stay strings — §5
//! re-specifies structure and validation, not weston's value syntaxes.

#![forbid(unsafe_code)]

mod cli;
mod model;
mod overrides;
mod resolve;

pub use cli::Cli;
pub use model::*;
pub use resolve::{Backend, ConfigError, Renderer, Settings, resolve, resolve_from};
