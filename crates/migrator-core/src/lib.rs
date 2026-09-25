//! Hermes Agent Migrator core.
//!
//! Platform-neutral migration engine shared by the GUI (Tauri) and CLI.
//! Never trusts a single absolute path, never deletes user data, and never
//! logs secrets.

pub mod checksum;
pub mod cloud;
pub mod cloudserver;
pub mod error;
pub mod installer;
pub mod manifest;
pub mod pack;
pub mod pathmapper;
pub mod platform;
pub mod remote;
pub mod restore;
pub mod scan;
pub mod secrets;

pub use error::MigratorError;

/// Layout of Hermes Agent as shipped in v0.20.x (the "HermesV1" layout).
/// If Hermes restructures `~/.hermes` in the future, add a HermesV2Adapter
/// and bump [`manifest::FORMAT_VERSION`].
pub const HERMES_LAYOUT_VERSION: u32 = 1;
pub const PACKAGE_EXT: &str = ".hermesmig";
