//! Code that only some configurations build. Each module holds two modules
//! that use each other: a module cycle, reported when the module is indexed.

#[cfg(windows)]
pub mod console;
#[cfg(feature = "export")]
pub mod export;
#[cfg(feature = "metrics")]
pub mod metrics;
