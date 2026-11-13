//! Chain transformers (`pathfinding/transformers/*`). Owner: **P4b**. Stateless free
//! functions (review 0: no per-instance caches), composed by `path::builder`.

pub mod component_check;
pub mod minimap;
pub mod shore;
pub mod smoothing;

#[cfg(test)]
mod tests;
