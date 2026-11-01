//! 街全体の状態。

use crate::command::{Command, MAX_TAX_PERMILLE, SimSpeed, ZoneKind};
use crate::state_hash::{StateHash, StateHasher};
use crate::units::{Money, Tick};

/// 始めたときの所持金。
pub const INITIAL_MONEY: Money = Money(500_000);

/// 始めたときの税率（‰）。
pub const INITIAL_TAX_PERMILLE: u16 = 100;

/// コマンドを適用した結果。どの結果でも状態ハッシュには影響しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 状態に反映した。
    Applied,
    /// 中核にまだ中身がない種類なので、何もしなかった。
    NotImplemented,
    /// 値が受け付ける範囲の外なので、何もしなかった。
    Rejected,
}

/// 街全体の状態の入れ物。
///
/// 状態を変えるのは [`World::apply`] と [`World::step`] だけである。
/// 1 tick の進め方は、その tick に記録されたコマンドを番号順に `apply` してから `step` を呼ぶ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    seed: u64,
    /// 次に進める tick。tick 0 から始め、`step` のたびに 1 増える。
    tick: Tick,
    speed: SimSpeed,
    money: Money,
    tax_permille: [u16; 4],
}

impl World {
    pub fn new(seed: u64) -> World {
        World {
            seed,
            tick: Tick::ZERO,
            speed: SimSpeed::NORMAL,
            money: INITIAL_MONEY,
            tax_permille: [INITIAL_TAX_PERMILLE; 4],
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// 次に進める tick。これまでに進めた tick 数でもある。
    pub fn tick(&self) -> Tick {
        self.tick
    }

    pub fn speed(&self) -> SimSpeed {
        self.speed
    }

    pub fn money(&self) -> Money {
        self.money
    }

    pub fn tax_permille(&self, zone: ZoneKind) -> u16 {
        self.tax_permille[zone.index()]
    }

    /// コマンドを 1 つ適用する。いまの tick の頭で呼ぶ。
    pub fn apply(&mut self, command: &Command) -> ApplyOutcome {
        match *command {
            Command::SetSimSpeed { speed } => {
                self.speed = speed;
                ApplyOutcome::Applied
            }
            Command::SetTaxRate { zone, permille } => {
                if permille > MAX_TAX_PERMILLE {
                    return ApplyOutcome::Rejected;
                }
                self.tax_permille[zone.index()] = permille;
                ApplyOutcome::Applied
            }
            Command::BuildRoad { .. }
            | Command::Bulldoze { .. }
            | Command::PaintZone { .. }
            | Command::PlaceBuilding { .. }
            | Command::SetSignal { .. }
            | Command::BuyChunk { .. } => ApplyOutcome::NotImplemented,
        }
    }

    /// 1 tick 進める。
    pub fn step(&mut self) {
        self.tick = self.tick.next();
    }

    /// 状態ハッシュ。表は決まった順番で書き出す。
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHasher::new();
        self.write_state(&mut h);
        h.finish()
    }
}

impl StateHash for World {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.seed);
        h.write(&self.tick);
        h.write(&self.speed.multiplier());
        h.write(&self.money);
        h.write(&self.tax_permille);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::ChunkCoord;

    #[test]
    fn step_advances_tick() {
        let mut w = World::new(1);
        assert_eq!(w.tick(), Tick::ZERO);
        w.step();
        w.step();
        assert_eq!(w.tick(), Tick(2));
    }

    #[test]
    fn hash_depends_on_seed_and_tick() {
        let a = World::new(1);
        let b = World::new(2);
        assert_ne!(a.state_hash(), b.state_hash());
        let mut c = a.clone();
        c.step();
        assert_ne!(a.state_hash(), c.state_hash());
    }

    #[test]
    fn set_tax_rate() {
        let mut w = World::new(0);
        let before = w.state_hash();
        let cmd = Command::SetTaxRate {
            zone: ZoneKind::Office,
            permille: 150,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::Applied);
        assert_eq!(w.tax_permille(ZoneKind::Office), 150);
        assert_eq!(w.tax_permille(ZoneKind::Residential), INITIAL_TAX_PERMILLE);
        assert_ne!(w.state_hash(), before);
    }

    #[test]
    fn out_of_range_tax_rate_is_rejected() {
        let mut w = World::new(0);
        let before = w.clone();
        let cmd = Command::SetTaxRate {
            zone: ZoneKind::Commercial,
            permille: MAX_TAX_PERMILLE + 1,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::Rejected);
        assert_eq!(w, before);
    }

    #[test]
    fn set_sim_speed() {
        let mut w = World::new(0);
        let before = w.state_hash();
        let speed = SimSpeed::new(8).unwrap();
        assert_eq!(
            w.apply(&Command::SetSimSpeed { speed }),
            ApplyOutcome::Applied
        );
        assert_eq!(w.speed(), speed);
        assert_ne!(w.state_hash(), before);
    }

    #[test]
    fn not_implemented_commands_leave_state_unchanged() {
        let mut w = World::new(0);
        let before = w.clone();
        let cmd = Command::BuyChunk {
            coord: ChunkCoord::new(1, -1),
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::NotImplemented);
        assert_eq!(w, before);
    }
}
