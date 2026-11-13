//! Config-independent EngineConfig / Config formulas (partial; the rest of EngineConfig is
//! game rules and belongs to the engine port, t065/t079). Checked against `config.json.zst`.

use crate::js;
use crate::util::{sigmoid, within};

const LN2: f64 = 0.6931471805599453; // Math.LN2

/// `trainSaturation(n)` (EngineConfig.ts:215).
pub fn train_saturation(n: f64) -> f64 {
    let damping = 1.0 - sigmoid(n, LN2 / 100.0, 560.0);
    let plateau = 0.25 * (1.0 - sigmoid(n, LN2 / 150.0, 900.0));
    js::max(damping, plateau)
}

/// `tradeShipSaturation(n)` (EngineConfig.ts:297).
pub fn trade_ship_saturation(n: f64) -> f64 {
    let damping = 1.0 - sigmoid(n, LN2 / 50.0, 330.0);
    let plateau = 0.25 * (1.0 - sigmoid(n, LN2 / 100.0, 800.0));
    js::max(damping, plateau)
}

/// `smallEconomySpawnBoost(levels)` (EngineConfig.ts:240).
pub fn small_economy_spawn_boost(levels: f64) -> f64 {
    if levels <= 0.0 {
        return 1.0;
    }
    1.0 + (0.5 * within((6.0 - levels) / 5.0, 0.0, 1.0)) / levels
}

/// `trainSpawnRate(numPlayerFactories, numTrainUnits)` (EngineConfig.ts:221).
pub fn train_spawn_rate(factories: f64, train_units: f64) -> f64 {
    let rate = (factories + 10.0) * 15.0;
    let boost = small_economy_spawn_boost(factories);
    js::max(1.0, (rate / (train_saturation(train_units) * boost)).floor())
}

/// `tradeShipSpawnRate(rejections, numTradeShips, playerPortLevels)` (EngineConfig.ts:312).
pub fn trade_ship_spawn_rate(rejections: f64, num_trade_ships: f64, port_levels: f64) -> f64 {
    let rejection_modifier = 1.0 / (rejections + 1.0);
    let boost = small_economy_spawn_boost(port_levels);
    let base = 100.0 / (boost * boost);
    js::max(1.0, ((base * rejection_modifier) / trade_ship_saturation(num_trade_ships)).floor())
}

/// `samRange(level)` (Config.ts:438): `maxSamRange() - 480 / (level + 5)`, maxSamRange = 150.
pub fn sam_range(level: f64) -> f64 {
    150.0 - 480.0 / (level + 5.0)
}
