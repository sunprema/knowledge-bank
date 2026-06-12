//! The two derived stores (persistence addendum §2): the turbovec vector
//! index (`index.tv`) and SQLite metadata (`meta.db`). Joined by
//! `chunks.id` = turbovec external id.

pub mod meta_db;
pub mod turbovec_index;

pub use meta_db::{MetaDb, NewChunk};
pub use turbovec_index::VectorIndex;
