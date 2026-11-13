//! `engine-api` ports (FILE_MAP rows 1-3): serde edge types and read-view traits. Owner: P1.
//! `map_files` (row 20, `MapFiles.ts`) belongs to P3.

pub mod game_config;
pub mod intents;
pub mod map_files;
pub mod types;
pub mod views;

pub use game_config::{GameConfig, GameStartInfo, GameStartPlayer};
pub use intents::{EmbargoAction, Intent, StampedIntent, Turn};
pub use types::{Difficulty, GameMapSize, GameMode, GameType, PlayerInfo, PlayerType, RankedType, UnitType};
pub use views::{PlayerLike, UnitLike};
