//! `Config` + `EngineConfig` (one struct, ARCHITECTURE §6), DoomsdayClock and Veterancy
//! formulas (FILE_MAP rows 23, 25, 31, 32). Owner: **P3**.

#[allow(clippy::module_inception)] // FILE_MAP row 23 names it `config::config` (Config.ts)
pub mod config;
pub mod doomsday;
pub mod engine_config;
#[cfg(test)]
mod vectors;
pub mod veterancy;

pub use engine_config::{AttackLogicInput, AttackLogicResult, EngineConfig};
