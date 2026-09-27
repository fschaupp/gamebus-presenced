//! The review edits, as values.
//!
//! Each verb is the whole decision; applying it is pure. The engine loads the
//! stash, calls [`apply_miss`] or [`apply_finding`] on one record, and saves.
//! Checks that need more than the record (an id's collision check against the
//! umu database) are the engine's, before it applies.
//!
//! Verbs are explicit rather than toggles, so a repeated call is idempotent.
//! Outcomes are structured rather than prose: each front-end words them its
//! own way.

use serde::{Deserialize, Serialize};

use crate::compat::{CompatFinding, TARGETS};
use crate::miss::{DraftBasis, DraftedId, Miss, Verification, VerificationState};

/// An edit to one identity miss. Every field it writes is annotation-half,
/// so a daemon write never reverts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum MissVerb {
    /// Park the entry: out of every export, kept in the stash.
    Dismiss,
    Undismiss,
    /// Opt the entry into the umu-database pipeline. umu misses only.
    Promote,
    Demote,
    /// Correct the title. The daemon's own resolution clears the override.
    SetTitle {
        title: String,
    },
    /// Correct the store. The daemon's own guess clears the override.
    SetStore {
        store: String,
    },
    /// Record what the game is on its store: a codename, and the store when
    /// the source named one. One verb, so the pair never lands half-applied.
    /// An identity, never a verdict.
    SetIdentity {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        store: Option<String>,
        codename: String,
    },
    /// Record a hand-typed umu id. The engine collision-checks it first.
    AssignId {
        id: String,
    },
    /// The user picked a umu-database row as "this game is that entry".
    PickEntry {
        store: String,
        codename: String,
        umu_id: String,
    },
}

/// What applying a [`MissVerb`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "kebab-case")]
pub enum MissChange {
    Dismissed,
    Restored,
    Promoted,
    Demoted,
    TitleSet {
        resolved: Option<String>,
    },
    /// The title matched the daemon's resolution; the override is gone.
    TitleReset,
    StoreSet {
        guessed: String,
    },
    /// The store matched the daemon's guess; the override is gone.
    StoreReset,
    IdentitySet,
    IdAssigned,
    /// The picked row is this entry's own store+codename: the launcher
    /// missed, not the database.
    AlreadyInDatabase,
    /// Another store's row: this copy shares its id.
    CrossStoreId,
}

/// Why a verb was refused. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "refusal", rename_all = "kebab-case")]
pub enum Refusal {
    /// A launcher launch never went through umu; there is nothing to promote.
    NotAUmuMiss,
    EmptyTitle,
    EmptyId,
    /// Not one of [`TARGETS`]. A typo recorded as a filing would leave the
    /// real target pending while the user believes it filed.
    UnknownTarget {
        target: String,
    },
}

/// Apply `verb` to `m`. `today` is `YYYY-MM-DD`, stamped where the verb
/// dates its annotation.
pub fn apply_miss(m: &mut Miss, verb: &MissVerb, today: &str) -> Result<MissChange, Refusal> {
    Ok(match verb {
        MissVerb::Dismiss => {
            // A repeat keeps the original date.
            m.dismissed.get_or_insert_with(|| today.to_string());
            MissChange::Dismissed
        }
        MissVerb::Undismiss => {
            m.dismissed = None;
            MissChange::Restored
        }
        MissVerb::Promote => {
            if !m.is_umu_miss() {
                return Err(Refusal::NotAUmuMiss);
            }
            m.umu_promoted.get_or_insert_with(|| today.to_string());
            MissChange::Promoted
        }
        MissVerb::Demote => {
            if !m.is_umu_miss() {
                return Err(Refusal::NotAUmuMiss);
            }
            m.umu_promoted = None;
            MissChange::Demoted
        }
        MissVerb::SetTitle { title } => {
            let title = title.trim();
            if title.is_empty() {
                return Err(Refusal::EmptyTitle);
            }
            if m.title.as_deref() == Some(title) {
                m.title_override = None;
                MissChange::TitleReset
            } else {
                m.title_override = Some(title.to_string());
                MissChange::TitleSet {
                    resolved: m.title.clone(),
                }
            }
        }
        MissVerb::SetStore { store } => {
            if *store == m.store {
                m.store_override = None;
                MissChange::StoreReset
            } else {
                m.store_override = Some(store.clone());
                MissChange::StoreSet {
                    guessed: m.store.clone(),
                }
            }
        }
        MissVerb::SetIdentity { store, codename } => {
            m.codename_override = Some(codename.clone());
            // A hand correction never wears a tool's provenance.
            m.codename_override_source = None;
            if let Some(s) = store {
                m.store_override = (*s != m.store).then(|| s.clone());
            }
            MissChange::IdentitySet
        }
        MissVerb::AssignId { id } => {
            let id = id.trim().to_lowercase();
            if id.is_empty() {
                return Err(Refusal::EmptyId);
            }
            m.drafted_id = Some(DraftedId {
                id,
                basis: DraftBasis::Manual,
                collision_checked: today.to_string(),
            });
            MissChange::IdAssigned
        }
        MissVerb::PickEntry {
            store,
            codename,
            umu_id,
        } => {
            let same_row = m.effective_store().eq_ignore_ascii_case(store)
                && m.effective_codename()
                    .is_some_and(|c| c.eq_ignore_ascii_case(codename));
            let state = if same_row {
                VerificationState::AlreadyInDatabase
            } else {
                VerificationState::CrossStoreId
            };
            m.verification = Some(Verification {
                state,
                umu_id: Some(umu_id.clone()),
                checked: today.to_string(),
                note: Some(format!(
                    "picked from the database's {store}/{codename} entry"
                )),
            });
            // The picked id supersedes any drafted one.
            m.drafted_id = None;
            if same_row {
                MissChange::AlreadyInDatabase
            } else {
                MissChange::CrossStoreId
            }
        }
    })
}

/// An edit to one compat finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "kebab-case")]
pub enum FindingVerb {
    /// Not worth filing: the finding stays, with its evidence, and stops
    /// being offered.
    Dismiss,
    Undismiss,
    /// Submitted to `target` (`protondb`, `awacy`, `gamedb`).
    MarkReported {
        target: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "kebab-case")]
pub enum FindingChange {
    Dismissed,
    Restored,
    Reported,
}

pub fn apply_finding(
    f: &mut CompatFinding,
    verb: &FindingVerb,
    today: &str,
) -> Result<FindingChange, Refusal> {
    Ok(match verb {
        FindingVerb::Dismiss => {
            f.dismissed.get_or_insert_with(|| today.to_string());
            FindingChange::Dismissed
        }
        FindingVerb::Undismiss => {
            f.dismissed = None;
            FindingChange::Restored
        }
        FindingVerb::MarkReported { target } => {
            if !TARGETS.contains(&target.as_str()) {
                return Err(Refusal::UnknownTarget {
                    target: target.clone(),
                });
            }
            f.reported.insert(target.clone(), today.to_string());
            FindingChange::Reported
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn miss(umu_id: &str) -> Miss {
        serde_json::from_value(serde_json::json!({
            "title": "Severed Steel",
            "store": "gog",
            "codename": "1242122770",
            "umu_id": umu_id,
            "title_source": "lutris-wrapper",
            "confidence": "medium",
            "executable": null,
            "first_seen": "2026-09-23",
            "last_seen": "2026-09-23",
        }))
        .expect("minimal miss")
    }

    #[test]
    fn dismiss_is_idempotent_and_keeps_the_first_date() {
        let mut m = miss("umu-default");
        apply_miss(&mut m, &MissVerb::Dismiss, "2026-09-01").unwrap();
        apply_miss(&mut m, &MissVerb::Dismiss, "2026-09-27").unwrap();
        assert_eq!(m.dismissed.as_deref(), Some("2026-09-01"));
        assert_eq!(
            apply_miss(&mut m, &MissVerb::Undismiss, "2026-09-27"),
            Ok(MissChange::Restored)
        );
        assert!(m.dismissed.is_none());
    }

    #[test]
    fn only_a_umu_miss_can_be_promoted_or_demoted() {
        let mut launcher = miss("");
        for verb in [MissVerb::Promote, MissVerb::Demote] {
            assert_eq!(
                apply_miss(&mut launcher, &verb, "2026-09-27"),
                Err(Refusal::NotAUmuMiss)
            );
        }
        assert!(launcher.umu_promoted.is_none());
        let mut m = miss("umu-default");
        assert_eq!(
            apply_miss(&mut m, &MissVerb::Promote, "2026-09-27"),
            Ok(MissChange::Promoted)
        );
        assert!(crate::umu_candidate(&m));
    }

    #[test]
    fn the_daemons_own_value_clears_an_override() {
        let mut m = miss("umu-default");
        let title = |t: &str| MissVerb::SetTitle { title: t.into() };
        assert_eq!(
            apply_miss(&mut m, &title("  Severed Steel "), "d"),
            Ok(MissChange::TitleReset)
        );
        assert!(m.title_override.is_none());
        assert_eq!(
            apply_miss(&mut m, &title(" "), "d"),
            Err(Refusal::EmptyTitle)
        );
        assert_eq!(
            apply_miss(
                &mut m,
                &MissVerb::SetStore {
                    store: "egs".into()
                },
                "d"
            ),
            Ok(MissChange::StoreSet {
                guessed: "gog".into()
            })
        );
        assert_eq!(
            apply_miss(
                &mut m,
                &MissVerb::SetStore {
                    store: "gog".into()
                },
                "d"
            ),
            Ok(MissChange::StoreReset)
        );
        assert!(m.store_override.is_none());
    }

    #[test]
    fn an_identity_lands_whole_and_drops_tool_provenance() {
        let mut m = miss("umu-default");
        m.codename_override_source = Some("lutris-library".into());
        let verb = MissVerb::SetIdentity {
            store: Some("egs".into()),
            codename: "Calluna".into(),
        };
        apply_miss(&mut m, &verb, "d").unwrap();
        assert_eq!(m.effective_store(), "egs");
        assert_eq!(m.effective_codename(), Some("Calluna"));
        assert!(m.codename_override_source.is_none());
        // An identity is not a verdict.
        assert!(m.verification.is_none());
    }

    #[test]
    fn a_pick_is_a_verdict_and_supersedes_the_draft() {
        let mut m = miss("umu-default");
        apply_miss(
            &mut m,
            &MissVerb::AssignId {
                id: " UMU-X ".into(),
            },
            "d",
        )
        .unwrap();
        assert_eq!(m.drafted_id.as_ref().unwrap().id, "umu-x");
        let pick = |store: &str| MissVerb::PickEntry {
            store: store.into(),
            codename: "1242122770".into(),
            umu_id: "umu-1227690".into(),
        };
        assert_eq!(
            apply_miss(&mut m, &pick("GOG"), "d"),
            Ok(MissChange::AlreadyInDatabase)
        );
        assert!(m.drafted_id.is_none());
        assert_eq!(
            apply_miss(&mut m, &pick("steam"), "d"),
            Ok(MissChange::CrossStoreId)
        );
    }

    #[test]
    fn finding_verbs_date_and_restore() {
        let mut f: CompatFinding = serde_json::from_value(serde_json::json!({
            "wall": "wine-stub",
            "first_seen": "2026-09-05",
            "last_seen": "2026-09-05",
        }))
        .unwrap();
        let report = |t: &str| FindingVerb::MarkReported { target: t.into() };
        apply_finding(&mut f, &report("protondb"), "2026-09-27").unwrap();
        assert_eq!(f.reported["protondb"], "2026-09-27");
        // A typo is refused, not recorded as a filing.
        assert_eq!(
            apply_finding(&mut f, &report("protndb"), "2026-09-27"),
            Err(Refusal::UnknownTarget {
                target: "protndb".into()
            })
        );
        assert_eq!(f.reported.len(), 1);
        apply_finding(&mut f, &FindingVerb::Dismiss, "2026-09-27").unwrap();
        assert!(!f.submittable());
        apply_finding(&mut f, &FindingVerb::Undismiss, "2026-09-27").unwrap();
        assert!(f.dismissed.is_none());
    }
}
