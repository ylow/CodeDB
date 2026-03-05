mod types;
mod builder;
mod vtab_helpers;
mod query_builder;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError};

pub struct TantivyVTab;

impl TantivyVTab {
    pub fn builder() -> TantivyVTabBuilder {
        TantivyVTabBuilder::new()
    }
}
