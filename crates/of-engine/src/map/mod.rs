//! Tile layer, map files and the unit grid (FILE_MAP rows 19-22, 26, 29-30). Owner: **P3**.

pub mod conn4;
pub mod game_map;
pub mod loader;
pub mod manifest;
pub mod maps_gen;
pub mod unit_grid;

pub use game_map::{GameMap, TileStateBuf};
pub use loader::MapStatics;
pub use unit_grid::UnitGrid;

#[cfg(test)]
mod tests;
