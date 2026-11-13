//! Water graph, components and the hierarchical water A* (FILE_MAP rows 35-42). Owner: **P4a**.
//! ARCHITECTURE §3.8 is the authority on what is state, shared value, or scratch.

pub mod abstract_graph;
pub mod astar_abstract;
pub mod bfs;
pub mod bfs_grid;
pub mod components;
pub mod hpa;
pub mod manager;
pub(crate) mod map_view;
pub mod priority_queue;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

pub use abstract_graph::{SegVisibility, WaterGraph};
pub use components::Components;
pub use manager::WaterManager;
