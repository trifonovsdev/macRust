//! The file manager: a Finder-style window for browsing folders. Everything
//! here is platform independent; the Windows host lives in
//! `crates/shell/src/win/files`.

pub mod art;
mod draw;
pub mod field;
pub mod format;
pub mod layout;
pub mod model;
pub mod strings;
pub mod view;

pub use view::{Action, Browser, Hit, IconRequest, Place, Section};
