mod types;
mod builder;

pub use types::{ColumnDef, ColumnSource};
pub use builder::{TantivyVTabBuilder, BuildError};

pub struct TantivyVTab;

impl TantivyVTab {
    pub fn builder() -> TantivyVTabBuilder {
        TantivyVTabBuilder::new()
    }
}
