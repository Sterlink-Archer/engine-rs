//! Rail network (`game/RailNetwork.ts` interface + impls, FILE_MAP rows 61-68). Owner: **P5**.
//! ARCHITECTURE §3.9. Ghost-rail preview (`computeGhostRailPaths`) is skipped (UI only).
//!
//! - [`network`] — `RailNetworkImpl` + `StationManagerImpl`
//! - [`station`] — `TrainStation` + `Cluster` (+ the stop handlers)
//! - [`railroad`] — `Railroad` + `OrientedRailroad`
//! - [`grid`] — `RailSpatialGrid`
//! - `env`: the `Game` surface the rail logic reads/writes (production impls on `State`/`Ctx`)
//! - `canon`: CS v1 export of the rail records and the four rail executions

mod canon;
pub(crate) mod env;
pub mod grid;
pub mod network;
pub mod railroad;
pub mod station;
#[cfg(test)]
pub(crate) mod testenv;

pub use network::{RailNetwork, StationManager};
pub use railroad::{OrientedRailroad, Railroad};
pub use station::{Cluster, TrainStation};
