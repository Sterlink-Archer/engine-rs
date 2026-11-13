//! `engine-lib/src/game/DoomsdayClock.ts`. Owner: **P3**. The pure formulas are already in
//! `of-det` (VECTORS `util`), re-exported here; the view-only `doomsdayClockWaveState` is not
//! ported.

pub use of_det::util::{
    DoomsdaySpeed, doomsday_clock_drain, doomsday_clock_required_tiles, doomsday_clock_rot_quota,
    doomsday_clock_troop_floor, rot_front_noise, rot_speckle_noise,
};

/// `ROT_NOISE_SCALE = 1 << 16` (`:240`).
pub const ROT_NOISE_SCALE: f64 = 65536.0;
