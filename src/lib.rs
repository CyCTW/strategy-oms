//! An embedded single-writer OMS. No network I/O, callbacks or threads are hidden
//! inside the reducer. Prices and quantities are fixed-point integer units.
mod core;
pub mod gateway;
mod group_model;
mod groups;
mod index;
pub mod journal;
pub mod model;
mod order_store;
mod pool;
mod price_pages;
mod price_tree;
mod single;
mod single_model;

pub use core::{Engine, Error, Limits, Outcome};
pub use group_model::*;
pub use index::{BookHandle, IndexBackend, IndexStats, LevelSummary, OrderIds, PriceLevel, Totals};
pub use model::*;
pub use single_model::*;
