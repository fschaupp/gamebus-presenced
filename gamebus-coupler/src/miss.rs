//! The identity-miss record: one launch with no authoritative identity, and
//! the annotations review adds to it. Persisted by the engine in
//! `umu-misses.json`; the shape here is that file's shape.

use serde::{Deserialize, Serialize};

/// How much to trust a resolved title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Launcher's own install record, or a detectable.json hit on the real
    /// game process - exact titles from curated sources.
    High,
    /// A wrapper-layer identification: the Lutris title argv, a descendant
    /// walk, a sandbox-family match. Human-set, occasionally edited.
    Medium,
    /// An MPRIS hint or an executable stem - better than nothing, verify
    /// before submitting.
    Low,
}

/// One observed launch with no authoritative identity, and what we made of
/// it. Originally only umu-database misses; a launch a launcher handed us
/// without a umu id is the same kind of gap and lives here too, with
/// `umu_id` empty ([`Miss::is_umu_miss`] tells the two apart).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Miss {
    /// Resolved display title, when anything resolved one.
    pub title: Option<String>,
    /// umu-database store id we believe this came from (`egs`, `gog`,
    /// `ubisoft`, … or `none`) - a guess, labelled as such.
    pub store: String,
    /// Store-internal codename when known (for EGS this is the App Name the
    /// database wants verbatim).
    pub codename: Option<String>,
    /// What umu reported (`umu-0` or `umu-default`) for a umu launch, or the
    /// empty string for a launch a launcher handed us with no umu id at all
    /// (see [`Miss::is_umu_miss`]). Defaulted on read so pre-widening files
    /// parse, and ALWAYS serialised so an older binary - which has no
    /// default - keeps parsing files this one writes.
    #[serde(default)]
    pub umu_id: String,
    /// Where the title came from (`heroic-config`, `detectable`,
    /// `lutris-wrapper`, `mpris-hint`, `stem`).
    pub title_source: Option<String>,
    pub confidence: Option<Confidence>,
    /// The game executable, when identified - feeds `EXE_STRINGS`/`NOTE`.
    pub executable: Option<String>,
    pub first_seen: String,
    pub last_seen: String,
    /// Which launcher handed us the launch (`lutris`, `heroic`), read off the
    /// merge-key prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    /// The launcher's own title for the game (Lutris `GAME_NAME`, the Heroic
    /// library title behind `HEROIC_APP_NAME` when it is known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_name: Option<String>,
    /// The launcher's install directory for the game (Lutris
    /// `GAME_DIRECTORY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher_dir: Option<String>,
    /// How the daemon learned `codename`: `lutris-config` (a
    /// `.lutrisgame.json`) or `heroic-env` (`HEROIC_APP_NAME`). Absent when
    /// nothing established it. A codename the SETUP TOOL fills lands on the
    /// annotation half instead (`codename_override` +
    /// `codename_override_source`): the annotator re-adopts this whole
    /// resolution half from disk before every persist, so its own write
    /// here would be flattened by its own save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename_source: Option<String>,
    /// Which runtime the launcher ran the game under: `native` or `proton`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    /// Result of checking this miss against the database. Absent until
    /// `gamebus-setup umu-misses --verify` runs; old stash files load fine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
    /// A proposed umu id, drafted per the database's own rules and
    /// collision-checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drafted_id: Option<DraftedId>,
    /// Set when an open upstream merge request already seems to contain this
    /// entry (`--check-prs`, best-effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub possible_pr: Option<String>,
    /// Whether this game has a protonfix upstream - the database's own scope
    /// rule. Absent until a `--verify` run could read the fix list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixCheck>,
    /// The user's store correction from the setup TUI. `store` stays the
    /// daemon's guess (its half of the entry); this override is annotation-
    /// half, so a daemon write never reverts it. Everything downstream reads
    /// [`Miss::effective_store`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_override: Option<String>,
    /// The user's codename correction from the setup TUI (a Heroic library
    /// pick or an online store lookup). `codename` stays the daemon's
    /// observation; this override is annotation-half like `store_override`,
    /// so a daemon write never reverts it. Everything downstream reads
    /// [`Miss::effective_codename`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename_override: Option<String>,
    /// Who supplied `codename_override` when it was a tool rather than the
    /// user: `lutris-library` (pga.db, matched on the launcher's own name
    /// and store). Cleared whenever the user types a codename themselves,
    /// so a hand correction never wears a tool's provenance. Annotation-half
    /// like the override it describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename_override_source: Option<String>,
    /// The user's title correction from the setup TUI (typed via `t`, or a
    /// GOG product lookup). `title` stays the daemon's resolution; this
    /// override is annotation-half like the other overrides, so a daemon
    /// write never reverts it. Everything downstream reads
    /// [`Miss::effective_title`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_override: Option<String>,
    /// Set (to the date) when the user dismissed this entry in the setup
    /// TUI: not wrong, just not wanted - dropped from the exports and
    /// parked at the bottom of the list. Annotation-half rather than a
    /// deletion, because the daemon's merge would resurrect a deleted key
    /// (and the next launch would re-record it anyway); a flag survives
    /// both. Reversible with the same key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<String>,

    /// The date the user promoted this entry into the umu-database pipeline
    /// (TUI key, annotation-half like `dismissed`). umu candidacy is opt-in:
    /// an entry qualifies when promoted here, or when verification matched
    /// an existing umu entry from another store (CrossStoreId), or when the
    /// protonfix check says the game needs umu. Everything else stays a
    /// gamedb-only identity record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub umu_promoted: Option<String>,
}

impl Miss {
    /// Whether this entry is a umu-database miss (umu ran and reported
    /// `umu-0`/`umu-default`) rather than a launch that never went through
    /// umu at all. Only the former belongs in a umu-database submission.
    pub fn is_umu_miss(&self) -> bool {
        !self.umu_id.is_empty()
    }

    /// The store every lookup and export should use: the user's correction
    /// when present, else the daemon's guess.
    pub fn effective_store(&self) -> &str {
        self.store_override.as_deref().unwrap_or(&self.store)
    }

    /// The codename every lookup and export should use: the user's
    /// correction when present, else the daemon's observation.
    pub fn effective_codename(&self) -> Option<&str> {
        self.codename_override
            .as_deref()
            .or(self.codename.as_deref())
    }

    /// The title every display, search, and export should use: the user's
    /// correction when present, else the daemon's resolution.
    pub fn effective_title(&self) -> Option<&str> {
        self.title_override.as_deref().or(self.title.as_deref())
    }
}

/// What checking a miss against the database established.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub state: VerificationState,
    /// The database's umu id, for the two states that found one.
    pub umu_id: Option<String>,
    pub checked: String,
    /// Anything the check wants a human to read - notably a discarded
    /// draft's collision ("umu-X already names Y").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationState {
    /// store+codename found - the launcher missed, not the database.
    AlreadyInDatabase,
    /// The title exists under another store; the database rule shares the id
    /// only when a Steam version exists, so this is a suggestion, not fact.
    CrossStoreId,
    /// Genuinely absent as of the check date.
    ConfirmedMissing,
}

/// Whether the game a miss names actually needs umu: the umu-database takes
/// entries for games that require a fix in Proton, and says so in its first
/// paragraph ("Games that run out of the box have no need be added"). A row
/// earns its place when a protonfix exists for its umu id and this store's
/// copy is not mapped to it yet; without a fix, submitting is noise for the
/// maintainers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixCheck {
    /// The umu id the check ran against - a later id change invalidates it.
    pub umu_id: String,
    /// The fix files found upstream, repository-relative
    /// (`gamefixes-steam/870780.py`). Empty means: no fix, out of scope.
    pub fixes: Vec<String>,
    /// Fixes in protonfixes' `localfixes` directory serving the same id,
    /// as absolute paths. Proof the game needs a fix, but not one a database
    /// row can point at until it is upstream.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local: Vec<String>,
    pub checked: String,
}

impl FixCheck {
    pub fn has_fix(&self) -> bool {
        !self.fixes.is_empty()
    }

    pub fn has_local_fix(&self) -> bool {
        !self.local.is_empty()
    }

    /// Upstream or local: either way the game needs a fix in Proton.
    pub fn needs_fix(&self) -> bool {
        self.has_fix() || self.has_local_fix()
    }
}

/// A proposed umu id and where it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftedId {
    pub id: String,
    pub basis: DraftBasis,
    pub collision_checked: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DraftBasis {
    /// detectable.json carries a Steam sku for the title → `umu-<appid>`,
    /// the id the database itself would assign.
    SteamSku,
    /// The store codename doubles as a product id and contains a letter
    /// (numeric ids would be parsed as SteamAppIds by Proton).
    StoreId,
    /// Made-up per the standalone rule: `umu-<title-slug>`, letters
    /// guaranteed.
    TitleSlug,
    /// Typed by the user in the setup TUI's misses pane - still
    /// collision-checked against the database before it is accepted.
    Manual,
}

/// Whether an entry participates in the umu-database pipeline at all.
///
/// Owner policy (2026-08-24): gamedb is ALWAYS active - every identity
/// record shows there by default - but umu-database participation is
/// OPT-IN per entry. An entry is a umu candidate only when it actually
/// went through umu ([`Miss::is_umu_miss`]) AND at least one of:
///
/// - the user promoted it in the TUI (`u`; `umu_promoted` carries the date),
/// - verification found the game already active in the umu database under
///   another store ([`VerificationState::CrossStoreId`] - the id exists,
///   this store's copy is the gap), or
/// - the fix check found a protonfix, upstream or local, i.e. the game
///   needs umu's help.
///
/// Suggestion and promotion, never automatic enrollment: everything else
/// stays a gamedb-only identity record, and drafting, `export::partition`,
/// both exporters, and `--check-prs` hold it back.
pub fn umu_candidate(m: &Miss) -> bool {
    m.is_umu_miss()
        && (m.umu_promoted.is_some()
            || m.verification
                .as_ref()
                .is_some_and(|v| v.state == VerificationState::CrossStoreId)
            || m.fix.as_ref().is_some_and(|f| f.needs_fix()))
}
