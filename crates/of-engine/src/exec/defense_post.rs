//! `DefensePostExecution` (`engine/src/execution/DefensePostExecution.ts`). Owner: **P7**.
//!
//! Fields: the `DefensePost v1 (execution)` snapshot record, in dump order
//! (`conformance/schema/snapshot_schemas_5b75bd3.txt`). `mg`/`map`/`config` game
//! references become the `cx: &mut Ctx` argument (PORTING_GUIDE §1).
//!
//! At the pin the targeting block of `tick` is commented out (`:52-95`), so `shoot()` has no
//! caller; it is ported anyway (literal, `#[allow(dead_code)]`) so a re-enabled upstream block
//! is a one-line change.

use crate::canon::{Canon, CanonCx, Cs};
use crate::exec::prelude::*;
use crate::exec::shell::ShellExec;

/// `DefensePostExecution` data.
#[derive(StateClone, Debug)]
pub struct DefensePostExec {
    pub active: bool,
    /// `initialized: this.mg !== undefined` (set by `init`).
    pub initialized: bool,
    pub post: UnitRef,
    pub target: Option<UnitRef>,
    pub last_shell_attack: i32,
    /// `alreadySentShell = new Set<Unit>()` (`:19`)
    pub already_sent_shell: JsSet<UnitRef>,
}

impl DefensePostExec {
    /// `new DefensePostExecution(post)`.
    pub fn new(post: UnitRef) -> Self {
        // TS: :14-19 `active = true`, `target = null`, `lastShellAttack = 0`, `new Set()`
        DefensePostExec {
            active: true,
            initialized: false,
            post,
            target: None,
            last_shell_attack: 0,
            already_sent_shell: JsSet::new(),
        }
    }

    /// `shoot()` (`:27-47`). No caller at the pin (module docs).
    #[allow(dead_code)]
    fn shoot(&mut self, cx: &mut Ctx) {
        // TS: :28
        let Some(target) = self.target else { return };
        // TS: :29-30
        let shell_attack_rate = cx.st.config().defense_post_shell_attack_rate();
        if f64::from(cx.st.ticks() - self.last_shell_attack) > shell_attack_rate {
            // TS: :31
            self.last_shell_attack = cx.st.ticks();
            // TS: :32-39 `new ShellExecution(post.tile(), post.owner(), post, target)`
            let (tile, owner) = {
                let p = cx.st.unit(self.post);
                (p.tile(), p.owner())
            };
            cx.st.add_execution(Exec::Shell(ShellExec::new(tile, owner, self.post, target)));
            // TS: :40-45
            if !cx.st.unit(target).has_health() {
                self.already_sent_shell.insert(target);
                self.target = None;
            }
        }
    }
}

impl ExecBehavior for DefensePostExec {
    fn is_active(&self) -> bool {
        self.active
    }
    /// `activeDuringSpawnPhase()` returns `false` (`DefensePostExecution.ts`).
    fn active_during_spawn_phase(&self) -> bool {
        false
    }
    /// TS: :23-25 `this.mg = mg`.
    fn init(&mut self, _cx: &mut Ctx, _ticks: i32) {
        self.initialized = true;
    }
    fn tick(&mut self, cx: &mut Ctx, _ticks: i32) {
        // TS: :50-53
        if !cx.st.unit(self.post).is_active() {
            self.active = false;
            return;
        }

        // TS: :55-58 (under construction: do nothing)
        if cx.st.unit(self.post).is_under_construction() {
            return;
        }

        // TS: :60-62
        if let Some(t) = self.target
            && !cx.st.unit(t).is_active()
        {
            self.target = None;
        }

        // TS: :64-106 commented out upstream (no ship targeting at the pin).
    }
}

/// `DefensePost v1`: `{ active, initialized, post, target|null, lastShellAttack,
/// alreadySentShell[] }` (`[...this.alreadySentShell]`: Set insertion order).
impl Canon for DefensePostExec {
    fn canon(&self, cx: &mut CanonCx<'_>) -> Cs {
        Cs::Obj(vec![
            ("active".into(), Cs::Bool(self.active)),
            ("initialized".into(), Cs::Bool(self.initialized)),
            ("post".into(), self.post.canon(cx)),
            ("target".into(), self.target.canon(cx)),
            ("lastShellAttack".into(), Cs::Int(i64::from(self.last_shell_attack))),
            ("alreadySentShell".into(), Cs::Arr(self.already_sent_shell.iter().map(|u| u.canon(cx)).collect())),
        ])
    }
}
