//! Game groups (S4f): the pure group model behind sticky representative
//! election.
//!
//! Every GameMode-sourced pid that probes to a merge key (`steam:<appid>`,
//! `lutris:<uuid>`, `umu:<id>`) becomes a **member** of that key's group. One
//! member - the **representative** - is forwarded to the correlator and
//! published as `pid_<rep>`; everyone else is absorbed silently, so wrapper
//! and helper churn never reaches the bus (R1).
//!
//! Core rules (spec §1):
//! - The first member becomes rep regardless of class; only a strictly
//!   higher-class member dethrones a live rep (R2). Depth never dethrones -
//!   it is a tie-breaker during election only.
//! - Rep loss is **deferred**: while any member holds evidence the record is
//!   held under the existing id ([`GameGroup::member_gone`] returns
//!   [`GroupEffect::Absorb`], never `Migrate`); the tick sweep elects a
//!   successor later. This kills the exit cascade (R3): the whole tree
//!   unregisters within milliseconds, before any tick, so the bus sees
//!   exactly one removal.
//! - Group identity is monotone: set once, upgradable Wrapper→GameProcess
//!   only, never cleared. An unidentified member can never rename an
//!   identified record.
//!
//! Like the correlator, this module is pure state + functions - zero `/proc`
//! I/O, no D-Bus (R7). Liveness is a fact recorded on [`Member`] by the
//! enricher's reconcile pass, not something this module discovers.

use std::collections::HashMap;

/// Maximum members tracked per group. A wrapper tree is typically well under
/// a dozen processes; the cap bounds memory against pathological registrants.
/// When exceeded, the lowest-class member is evicted first (never the rep).
pub const MAX_GROUP_MEMBERS: usize = 32;

/// How much a member looks like the actual game, ascending priority.
/// The derived `Ord` follows declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberClass {
    /// Known wrapper executable (reaper, srt-bwrap, pressure-vessel, ...).
    Helper,
    /// Non-wrapper executable, unidentified.
    Plain,
    /// `identify_wrapper` resolved the group identity through this pid.
    IdentifiedWrapper,
    /// `identify_process` hit, or exe under `/steamapps/` with a resolvable
    /// Steam appid - the real game process.
    GameProcess,
}

/// Confidence class of a group identity (spec §1.3), ascending priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdentityClass {
    /// Resolved through a wrapper (launch script, umu id, Wine cmdline).
    Wrapper,
    /// Resolved from the game process itself.
    GameProcess,
}

/// A resolved group identity: the name/exe the published record carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub exe: String,
    pub class: IdentityClass,
}

/// One process in a group and the evidence it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// GameMode currently has this pid registered.
    pub gamemode: bool,
    /// The `/proc` scan found this pid carrying the group's key.
    pub scan: bool,
    pub class: MemberClass,
    /// Process-tree depth below the shallowest known ancestor.
    pub depth: usize,
    /// `/proc/<pid>/stat` start time at insert - the pid-reuse guard.
    pub start_time: Option<u64>,
    /// `/proc/<pid>` existed (with the key intact) at the last refresh.
    pub alive: bool,
}

impl Member {
    /// Active evidence per spec §1.2: a gamemode registration, or scan
    /// evidence with the process still alive (and still carrying the key -
    /// the enricher's liveness refresh folds that into `alive`).
    fn evidenced(&self) -> bool {
        self.gamemode || (self.scan && self.alive)
    }
}

/// All members sharing one merge key, plus the published-record state.
#[derive(Debug, Clone)]
pub struct GameGroup {
    pub key: String,
    pub members: HashMap<u32, Member>,
    /// The pid whose `pid_<rep>` id is on the bus. 0 until the first upsert.
    pub rep: u32,
    pub identity: Option<Identity>,
    /// Group creation time - `Since` never jumps across migrations.
    pub since: i64,
    pub steam_appid: Option<String>,
}

/// What the enricher must forward to the correlator after a group input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupEffect {
    /// Forward the rep's activity as today (new rep, or rep update).
    PublishRep,
    /// Recorded as a member; forward nothing. The bus never sees the pid.
    Absorb,
    /// New rep dethroned `old`: emit the publish-first four-event sequence.
    Migrate { old: u32 },
    /// Last evidence gone: remove the group's records from the bus.
    RemoveAll,
}

impl GameGroup {
    /// A group with no members yet; the first [`upsert`](Self::upsert)
    /// installs the rep.
    pub fn new(key: impl Into<String>, since: i64) -> Self {
        Self {
            key: key.into(),
            members: HashMap::new(),
            rep: 0,
            identity: None,
            since,
            steam_appid: None,
        }
    }

    /// Insert or refresh a member (spec §1.1).
    ///
    /// The first member becomes rep regardless of class. A later member
    /// dethrones only with a **strictly greater** class - equal or lower is
    /// absorbed; depth never dethrones a live rep. `rep_pinned` is the
    /// Discord exception (spec §1.2): a rep carrying a joined Discord
    /// partial is displaced by rep death only, never by class.
    pub fn upsert(&mut self, pid: u32, member: Member, rep_pinned: bool) -> GroupEffect {
        if self.members.is_empty() {
            self.rep = pid;
            self.members.insert(pid, member);
            return GroupEffect::PublishRep;
        }
        if pid == self.rep {
            self.members.insert(pid, member);
            return GroupEffect::PublishRep;
        }

        // A missing rep entry cannot happen through this module's own
        // transitions (the rep is never evicted or dropped); treat it as
        // dethronable so the group self-heals instead of wedging.
        let dethrones = match self.members.get(&self.rep) {
            Some(rep_member) => member.class > rep_member.class,
            None => true,
        };
        let effect = if dethrones && !rep_pinned {
            let old = self.rep;
            self.rep = pid;
            GroupEffect::Migrate { old }
        } else {
            GroupEffect::Absorb
        };
        self.members.insert(pid, member);
        self.enforce_cap();
        effect
    }

    /// GameMode stopped asserting `pid` (spec §1.2, deferred migration).
    ///
    /// Non-rep members were never on the bus: bookkeeping only. For the rep,
    /// the gamemode flag is cleared and the record is **held** under the
    /// existing id while any member still carries evidence - this returns
    /// [`GroupEffect::Absorb`], never `Migrate` (the tick sweep elects a
    /// successor later). Only when the last evidence is gone does it return
    /// [`GroupEffect::RemoveAll`].
    pub fn member_gone(&mut self, pid: u32) -> GroupEffect {
        if pid != self.rep {
            if let Some(member) = self.members.get_mut(&pid) {
                member.gamemode = false;
                // No scan evidence left either: forget the member entirely.
                if !member.scan {
                    self.members.remove(&pid);
                }
            }
            return GroupEffect::Absorb;
        }

        if let Some(member) = self.members.get_mut(&pid) {
            member.gamemode = false;
        }
        if self.has_evidence() {
            GroupEffect::Absorb
        } else {
            GroupEffect::RemoveAll
        }
    }

    /// Elect the best successor rep (spec §1.2): among `alive && evidenced`
    /// members, highest class first; ties by depth - deepest within
    /// GameProcess (the game sits at the bottom of the wrapper tree),
    /// shallowest otherwise (longest-lived wrapper); final tie lowest pid.
    pub fn elect(&self) -> Option<u32> {
        self.members
            .iter()
            .filter(|(_, m)| m.alive && m.evidenced())
            .max_by(|(pid_a, a), (pid_b, b)| {
                a.class
                    .cmp(&b.class)
                    .then_with(|| depth_preference(a).cmp(&depth_preference(b)))
                    // Lowest pid wins the final tie: reversed for max_by.
                    .then_with(|| pid_b.cmp(pid_a))
            })
            .map(|(pid, _)| *pid)
    }

    /// Set the group identity, monotonically (spec §1.3): set once,
    /// upgradable Wrapper→GameProcess only, never cleared or overwritten
    /// sideways.
    pub fn set_identity(&mut self, identity: Identity) {
        match &self.identity {
            None => self.identity = Some(identity),
            Some(current) if identity.class > current.class => self.identity = Some(identity),
            Some(_) => {}
        }
    }

    /// Does any member still hold active evidence for the game?
    pub fn has_evidence(&self) -> bool {
        self.members.values().any(Member::evidenced)
    }

    /// Evict lowest-class members (never the rep) beyond the cap.
    fn enforce_cap(&mut self) {
        while self.members.len() > MAX_GROUP_MEMBERS {
            let victim = self
                .members
                .iter()
                .filter(|(pid, _)| **pid != self.rep)
                .min_by_key(|(pid, m)| (m.class, **pid))
                .map(|(pid, _)| *pid);
            match victim {
                Some(pid) => {
                    self.members.remove(&pid);
                }
                None => break,
            }
        }
    }
}

/// Depth ranking within one class, "greater is better": deepest wins inside
/// GameProcess, shallowest wins elsewhere. Only ever compared between
/// members of equal class (class is compared first).
fn depth_preference(member: &Member) -> i64 {
    match member.class {
        MemberClass::GameProcess => member.depth as i64,
        _ => -(member.depth as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live, gamemode-registered member of the given class and depth.
    fn member(class: MemberClass, depth: usize) -> Member {
        Member {
            gamemode: true,
            scan: false,
            class,
            depth,
            start_time: Some(1000),
            alive: true,
        }
    }

    #[test]
    fn identified_rep_never_displaced_by_deeper_unidentified() {
        // The Amnesia kill shot: identified wrapper 708904 holds the record;
        // deeper unidentified pids must be absorbed, never replace it.
        let mut g = GameGroup::new("lutris:1cf08001", 1_700_000_000);
        assert_eq!(
            g.upsert(708904, member(MemberClass::IdentifiedWrapper, 0), false),
            GroupEffect::PublishRep
        );
        assert_eq!(
            g.upsert(708947, member(MemberClass::Plain, 4), false),
            GroupEffect::Absorb
        );
        assert_eq!(
            g.upsert(708989, member(MemberClass::Helper, 6), false),
            GroupEffect::Absorb
        );
        assert_eq!(g.rep, 708904);
        // A GameProcess member IS strictly greater: one Migrate, publish-first.
        assert_eq!(
            g.upsert(709001, member(MemberClass::GameProcess, 8), false),
            GroupEffect::Migrate { old: 708904 }
        );
        assert_eq!(g.rep, 709001);
    }

    #[test]
    fn equal_class_never_displaces_live_rep() {
        // Hysteresis (Brotato helpers): srt-bwrap arriving after reaper is
        // the same class - absorb, no churn. Same for equal wrappers.
        let mut g = GameGroup::new("steam:1942280", 1_700_000_000);
        g.upsert(698374, member(MemberClass::Helper, 0), false);
        assert_eq!(
            g.upsert(698398, member(MemberClass::Helper, 1), false),
            GroupEffect::Absorb
        );
        assert_eq!(g.rep, 698374);

        let mut g = GameGroup::new("lutris:abc", 0);
        g.upsert(1, member(MemberClass::IdentifiedWrapper, 0), false);
        assert_eq!(
            g.upsert(2, member(MemberClass::IdentifiedWrapper, 3), false),
            GroupEffect::Absorb
        );
        assert_eq!(g.rep, 1);
    }

    #[test]
    fn rep_death_holds_record_while_evidence_survives() {
        // Exit-cascade guard: member_gone returns Absorb (hold the record),
        // never Migrate - migration is the tick sweep's job.
        let mut g = GameGroup::new("steam:1942280", 0);
        g.upsert(10, member(MemberClass::Helper, 0), false);
        g.upsert(11, member(MemberClass::Helper, 1), false);
        assert_eq!(g.member_gone(10), GroupEffect::Absorb);
        assert_eq!(g.rep, 10, "record stays under the existing pid_<rep> id");
        assert!(!g.members.get(&10).unwrap().gamemode);
        assert!(g.has_evidence());

        // Scan evidence with a live process also holds the record.
        let mut g = GameGroup::new("steam:1942280", 0);
        g.upsert(20, member(MemberClass::Helper, 0), false);
        let scan_only = Member {
            gamemode: false,
            scan: true,
            ..member(MemberClass::GameProcess, 3)
        };
        g.upsert(21, scan_only, false);
        assert_eq!(g.member_gone(20), GroupEffect::Absorb);
    }

    #[test]
    fn remove_all_only_when_last_evidence_gone() {
        // A bare-alive member with no sources (gamemode nor scan) is not
        // evidence and must not block RemoveAll.
        let mut g = GameGroup::new("steam:90001", 0);
        g.upsert(30, member(MemberClass::Helper, 0), false);
        let bare = Member {
            gamemode: false,
            scan: false,
            ..member(MemberClass::Helper, 2)
        };
        assert_eq!(g.upsert(31, bare, false), GroupEffect::Absorb);
        assert_eq!(g.member_gone(30), GroupEffect::RemoveAll);

        // With a registered survivor the same removal holds instead.
        let mut g = GameGroup::new("steam:90001", 0);
        g.upsert(40, member(MemberClass::Helper, 0), false);
        g.upsert(41, member(MemberClass::Helper, 2), false);
        assert_eq!(g.member_gone(40), GroupEffect::Absorb);
        // Last evidence goes with the non-rep survivor (bookkeeping only -
        // the tick sweep turns the evidence-less group into a RemoveAll).
        assert_eq!(g.member_gone(41), GroupEffect::Absorb);
        assert!(!g.has_evidence());
    }

    #[test]
    fn election_ordering_truth_table() {
        // class > (deepest within GameProcess / shallowest otherwise) >
        // lowest pid; dead or evidence-less members are ineligible.
        let mut g = GameGroup::new("steam:1", 0);
        g.members.insert(10, member(MemberClass::Helper, 1));
        g.members.insert(11, member(MemberClass::Plain, 0));
        g.members
            .insert(12, member(MemberClass::IdentifiedWrapper, 3));
        g.members
            .insert(13, member(MemberClass::IdentifiedWrapper, 1));
        g.members.insert(14, member(MemberClass::GameProcess, 2));
        g.members.insert(15, member(MemberClass::GameProcess, 4));
        // Ineligible: dead, and alive-but-evidence-less.
        let dead = Member {
            alive: false,
            ..member(MemberClass::GameProcess, 9)
        };
        g.members.insert(16, dead);
        let no_evidence = Member {
            gamemode: false,
            scan: false,
            ..member(MemberClass::GameProcess, 9)
        };
        g.members.insert(17, no_evidence);

        // Highest class wins; within GameProcess the deepest.
        assert_eq!(g.elect(), Some(15));
        g.members.remove(&15);
        assert_eq!(g.elect(), Some(14));
        // Within non-GameProcess classes the shallowest (longest-lived) wins.
        g.members.remove(&14);
        assert_eq!(g.elect(), Some(13));
        g.members.remove(&13);
        g.members.remove(&12);
        assert_eq!(g.elect(), Some(11), "Plain outranks Helper");
        // Full tie: lowest pid, deterministically.
        g.members.insert(21, member(MemberClass::Plain, 0));
        g.members.insert(20, member(MemberClass::Plain, 0));
        assert_eq!(g.elect(), Some(11));
        g.members.remove(&11);
        assert_eq!(g.elect(), Some(20));
        // Nobody eligible left: no successor.
        g.members.retain(|pid, _| [16, 17].contains(pid));
        assert_eq!(g.elect(), None);
    }

    #[test]
    fn identity_monotone() {
        let wrapper = |name: &str| Identity {
            name: name.to_string(),
            exe: "/usr/bin/umu-run".to_string(),
            class: IdentityClass::Wrapper,
        };
        let game = |name: &str| Identity {
            name: name.to_string(),
            exe: "/games/AmnesiaTheBunker.exe".to_string(),
            class: IdentityClass::GameProcess,
        };

        let mut g = GameGroup::new("lutris:1cf08001", 0);
        g.set_identity(wrapper("Amnesia: The Bunker"));
        // Same class never overwrites (set once).
        g.set_identity(wrapper("i386-linux-gnu-inspect-library"));
        assert_eq!(g.identity.as_ref().unwrap().name, "Amnesia: The Bunker");
        // Wrapper -> GameProcess upgrades.
        g.set_identity(game("Amnesia: The Bunker"));
        assert_eq!(
            g.identity.as_ref().unwrap().class,
            IdentityClass::GameProcess
        );
        // GameProcess is final: no sideways churn, never cleared.
        g.set_identity(game("Other"));
        g.set_identity(wrapper("Other"));
        assert_eq!(g.identity.as_ref().unwrap().name, "Amnesia: The Bunker");

        // Downgrade attempt on a fresh GameProcess identity is ignored too.
        let mut g = GameGroup::new("steam:1942280", 0);
        g.set_identity(game("Brotato"));
        g.set_identity(wrapper("reaper"));
        assert_eq!(g.identity.as_ref().unwrap().name, "Brotato");
    }

    #[test]
    fn n_members_one_publish() {
        // S4e explosion regression guard: N same-class registrations produce
        // exactly one PublishRep, everything else absorbs.
        let mut g = GameGroup::new("steam:1942280", 0);
        let mut publishes = 0;
        for pid in 100..120 {
            let effect = g.upsert(
                pid,
                member(MemberClass::Helper, (pid - 100) as usize),
                false,
            );
            if effect == GroupEffect::PublishRep {
                publishes += 1;
            } else {
                assert_eq!(effect, GroupEffect::Absorb);
            }
        }
        assert_eq!(publishes, 1);
        assert_eq!(g.rep, 100);
    }

    #[test]
    fn member_cap_drops_lowest_class_first() {
        let mut g = GameGroup::new("steam:1", 0);
        g.upsert(1, member(MemberClass::Plain, 0), false); // rep
        for pid in 2..=31u32 {
            g.upsert(pid, member(MemberClass::Plain, 1), false);
        }
        g.upsert(500, member(MemberClass::Helper, 1), false);
        assert_eq!(g.members.len(), MAX_GROUP_MEMBERS);

        // One over the cap: the lone Helper is the lowest class and goes.
        g.upsert(600, member(MemberClass::Plain, 1), false);
        assert_eq!(g.members.len(), MAX_GROUP_MEMBERS);
        assert!(!g.members.contains_key(&500));
        assert!(g.members.contains_key(&600));

        // All equal now: lowest pid among the lowest class goes - but never
        // the rep, even when it is the lowest-class lowest pid.
        g.upsert(601, member(MemberClass::Plain, 1), false);
        assert_eq!(g.members.len(), MAX_GROUP_MEMBERS);
        assert!(g.members.contains_key(&1), "rep is never evicted");
        assert!(!g.members.contains_key(&2));
        assert_eq!(g.rep, 1);
    }

    #[test]
    fn rep_pinned_blocks_class_dethrone() {
        // Discord exception: a pinned rep is displaced by death only.
        let mut g = GameGroup::new("steam:1", 0);
        g.upsert(50, member(MemberClass::Helper, 0), false);
        assert_eq!(
            g.upsert(51, member(MemberClass::GameProcess, 3), true),
            GroupEffect::Absorb
        );
        assert_eq!(g.rep, 50);
    }
}
