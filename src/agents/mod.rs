//! Multi-agent layer: specialist agents that reason over the corpus together.
//!
//! [`roundtable`] runs a panel of personas (technologist, business, skeptic, …)
//! through N rounds of debate on a startup objective, grounded in the knowledge
//! bank, and emits a live event stream so a UI can render the discussion as it
//! unfolds (the macOS app's Roundtable canvas).

pub mod roundtable;
