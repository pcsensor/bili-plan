//! Pure planning and scheduling rules shared by desktop and server.
//!
//! Dependency direction: applications -> domain. This crate must not import
//! UI frameworks, HTTP clients, databases, or either application crate.

pub mod catalog;
pub mod model;
pub mod plan;
pub mod schedule_recovery;
pub mod study;
