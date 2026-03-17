pub mod protocol;
pub mod schema;
pub mod types;

// Re-export core types at crate root for convenience
pub use types::{
    Action, ActionFilter, ActivityLevel, Confidence, FileEvent, Filter, HotFile, Pin, Preference,
    Source, Summary,
};
