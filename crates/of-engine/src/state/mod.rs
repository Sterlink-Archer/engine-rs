//! Simulation state (`GameImpl` data and the entity types). ARCHITECTURE §3.
//!
//! Every type here derives `StateClone` (pooled forks) and is plain data: no `Rc`,
//! `RefCell`, `Box<dyn>` (enforced by `PlainState`, see [`crate::plain`]).
//!
//! TS `TypeError` sites use `.ts_expect("<file:line>")`, never `unwrap`/`expect`
//! (PORTING_GUIDE §1, §12); the lints below make a bare one a compile error here. A true
//! Rust invariant uses `#[allow(..)]` with a `// rust-invariant:` comment.
//!
//! | module | TS | owner |
//! |---|---|---|
//! | `game` | `GameImpl`, `Game.ts` (`Game`) | P1 |
//! | `alliance`, `alliance_request` | `AllianceImpl`, `AllianceRequestImpl` | P1 |
//! | `memo`, `gc` | `unitsByTypeMemo`/`unitCountMemo`; arena GC (§3.11) | P1 |
//! | `player`, `unit`, `stats` | `PlayerImpl`, `UnitImpl`, `StatsImpl`/`Stats.ts`/`StatsSchemas` | P2 |
//! | `attack` | `AttackImpl` | P6 |
//! | `transport_ship_utils` | `TransportShipUtils` | P7 |
//! | `shared_water_cache` | `SharedWaterCache` | P11 |
//!
//! Owner: **P1** (FILE_MAP §2 foundations).
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod alliance;
pub mod alliance_request;
pub mod attack;
pub mod game;
pub mod gc;
pub mod memo;
pub mod player;
pub mod shared_water_cache;
pub mod stats;
pub mod transport_ship_utils;
pub mod unit;

pub use game::{ShipKind, State, Winner, create_state, create_state_restoring, human_player_info};
