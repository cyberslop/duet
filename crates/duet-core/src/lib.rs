//! duet — a cross-model adversarial development loop. One model builds, the other
//! adversarially critiques, iterating over a git repo until the critic signs off.
//! See `orchestrate::execute` for the engine and `events` for the typed,
//! provider-neutral event model that powers the live conversation view.

pub mod advisor;
pub mod agent;
pub mod catalog;
pub mod domain;
pub mod events;
pub mod hardware;
pub mod git;
pub mod orchestrate;
pub mod prompts;
pub mod report;
pub mod render;
pub mod style;

/// The JSON schema critics emit (shared by the engine and the local critic).
pub const REVIEW_SCHEMA: &str = include_str!("../schemas/review.json");
