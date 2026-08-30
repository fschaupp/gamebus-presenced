//! The umu-database miss report.
//!
//! Every game launched through umu with no database entry (`GAMEID=umu-0`,
//! `UMU_ID=umu-default`) is a gap in the shared umu-database - and this
//! daemon usually works out what the game actually was. This module stashes
//! those resolutions so the setup tool can show them for review and export a
//! submission in the database's own CSV shape
//! (https://github.com/Open-Wine-Components/umu-database:
//! `TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM,NOTE,EXE_STRINGS`).
//!
//! The stash has since widened past umu: a launch a launcher handed us with
//! no authoritative identity is the same kind of gap, and is recorded here
//! with an empty `umu_id` (see [`Miss::is_umu_miss`]). Only umu misses can
//! become a umu-database submission; the rest still carry launcher facts
//! worth showing and correcting.
//!
//! Persisted at `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json` - data,
//! not cache: it accumulates across sessions and is the raw material for a
//! human-reviewed contribution. Two writers share it, each owning half of
//! every entry: the daemon writes resolutions, the setup tool writes
//! verification annotations; every persist merges the other half from
//! disk first (see [`UmuReport`]). Writes are atomic. The daemon stays
//! network-free; submitting is the user's act, not ours.

// Compiled into both the daemon (which uses the write half) and gamebus-setup
// (which reads the stash and owns the whole UmuDb/verification half - the
// daemon is network-free and never touches the database). Each binary alone
// reports the other's subset as dead; every item is live
// in at least one binary.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Whether this game has a protonfix upstream — the database's own scope
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

/// What the launcher said about a launch, as far as the daemon could read
/// it off the environment. All optional, all resolution-half: a launcher
/// that stays quiet contributes `None`, and `None` never clears a fact an
/// earlier launch established (see
/// [`UmuReport::note_launch_with`](UmuReport::note_launch_with)).
#[derive(Debug, Clone, Default)]
pub struct LaunchFacts {
    /// `lutris` or `heroic` - the merge-key prefix.
    pub launcher: Option<String>,
    /// The launcher's own title for the game.
    pub launcher_name: Option<String>,
    /// The launcher's install directory for the game.
    pub launcher_dir: Option<String>,
    /// How the codename was learned (`lutris-config`, `heroic-env`,
    /// `lutris-library`).
    pub codename_source: Option<String>,
    /// `native` or `proton`.
    pub runner: Option<String>,
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
    /// The umu id the check ran against — a later id change invalidates it.
    pub umu_id: String,
    /// The fix files found upstream, repository-relative
    /// (`gamefixes-steam/870780.py`). Empty means: no fix, out of scope.
    pub fixes: Vec<String>,
    pub checked: String,
}

impl FixCheck {
    pub fn has_fix(&self) -> bool {
        !self.fixes.is_empty()
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

/// The stash: keyed by `store:codename` (or the merge key when no codename
/// exists), loaded once, written through on change.
///
/// TWO writers share the file, each owning half of every entry: the daemon
/// writes the resolution half (title, store, codename, confidence, the
/// launcher facts, …), the setup tool writes the annotation half
/// (`verification`, `drafted_id`, `possible_pr`, the user's `*_override`
/// corrections). Every persist re-reads the file and adopts the other
/// writer's half first, so a daemon launch after `--verify` keeps the
/// verdicts and a `--verify` during a session keeps fresh misses.
#[derive(Debug, Default)]
pub struct UmuReport {
    entries: HashMap<String, Miss>,
    path: Option<PathBuf>,
    dirty: bool,
    /// True in the setup tool: this instance owns the annotation half.
    annotator: bool,
    /// Set when the file exists but did not parse. The path is dropped in
    /// that case so no write can ever flatten a file we failed to read -
    /// accumulated knowledge beats a working session.
    load_error: Option<String>,
}

impl UmuReport {
    /// Load from `$XDG_DATA_HOME/gamebus-presenced/umu-misses.json`.
    pub fn load() -> Self {
        match Self::default_path() {
            Some(path) => Self::from_path(path),
            None => Self::default(),
        }
    }

    /// The setup tool's constructor: same file, annotation half owned.
    pub fn load_for_annotations() -> Self {
        Self {
            annotator: true,
            ..Self::load()
        }
    }

    /// crate-visible for tests that need a stash on a scratch path.
    pub(crate) fn from_path(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            // Missing file: a fresh stash that writes normally.
            Err(_) => Self {
                path: Some(path),
                ..Self::default()
            },
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(entries) => Self {
                    entries,
                    path: Some(path),
                    ..Self::default()
                },
                Err(e) => Self {
                    load_error: Some(format!(
                        "{} exists but failed to parse: {e}",
                        path.display()
                    )),
                    ..Self::default()
                },
            },
        }
    }

    /// Why the stash refused to load, if it did. Callers surface this -
    /// silently showing "no misses" over a corrupt file hides data loss.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn default_path() -> Option<PathBuf> {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
        Some(data.join("gamebus-presenced").join("umu-misses.json"))
    }

    /// A launch worth stashing was observed. Creates or refreshes the entry;
    /// the title may arrive later via [`note_title`].
    ///
    /// `umu_id` is what umu reported (`umu-0`/`umu-default`), or `""` for a
    /// launch that never went through umu.
    pub fn note_launch(
        &mut self,
        store: &str,
        codename: Option<&str>,
        umu_id: &str,
        fallback_key: &str,
    ) {
        self.note_launch_with(
            store,
            codename,
            umu_id,
            fallback_key,
            LaunchFacts::default(),
        );
    }

    /// [`note_launch`](Self::note_launch) plus whatever the launcher said
    /// about the game. Every fact is optional and only ever ADDS: a fact
    /// this launch does not carry leaves an earlier launch's value standing,
    /// so a later, thinner environment can never blank the stash.
    pub fn note_launch_with(
        &mut self,
        store: &str,
        codename: Option<&str>,
        umu_id: &str,
        fallback_key: &str,
        facts: LaunchFacts,
    ) {
        let key = Self::entry_key(store, codename, fallback_key);
        let today = today();
        let entry = self.entries.entry(key).or_insert_with(|| Miss {
            title: None,
            store: store.to_string(),
            codename: codename.map(str::to_string),
            umu_id: umu_id.to_string(),
            title_source: None,
            confidence: None,
            executable: None,
            first_seen: today.clone(),
            last_seen: today.clone(),
            launcher: None,
            launcher_name: None,
            launcher_dir: None,
            codename_source: None,
            runner: None,
            verification: None,
            drafted_id: None,
            possible_pr: None,
            fix: None,
            store_override: None,
            codename_override: None,
            codename_override_source: None,
            title_override: None,
            dismissed: None,
            umu_promoted: None,
        });
        entry.last_seen = today;
        fill(&mut entry.launcher, facts.launcher);
        fill(&mut entry.launcher_name, facts.launcher_name);
        fill(&mut entry.launcher_dir, facts.launcher_dir);
        fill(&mut entry.codename_source, facts.codename_source);
        fill(&mut entry.runner, facts.runner);
        self.dirty = true;
        self.persist();
    }

    /// A title (or a better title) resolved for a missed launch. Upgrades
    /// only: an existing higher-confidence resolution is never overwritten
    /// by a weaker one.
    #[allow(clippy::too_many_arguments)]
    pub fn note_title(
        &mut self,
        store: &str,
        codename: Option<&str>,
        fallback_key: &str,
        title: &str,
        source: &str,
        confidence: Confidence,
        executable: Option<&str>,
    ) {
        if title.is_empty() {
            return;
        }
        let key = Self::entry_key(store, codename, fallback_key);
        let Some(entry) = self.entries.get_mut(&key) else {
            return; // note_launch records the miss first; no launch, no entry.
        };
        let stronger = match (entry.confidence, confidence) {
            (None, _) => true,
            (Some(old), new) => rank(new) > rank(old),
        };
        let same_but_fresher =
            entry.confidence == Some(confidence) && entry.title.as_deref() != Some(title);
        if stronger || same_but_fresher {
            entry.title = Some(title.to_string());
            entry.title_source = Some(source.to_string());
            entry.confidence = Some(confidence);
            if let Some(exe) = executable {
                if !exe.is_empty() {
                    entry.executable = Some(exe.to_string());
                }
            }
            self.dirty = true;
            self.persist();
        }
    }

    /// Read access for the setup tool's review/verify/export flows.
    pub fn entries(&self) -> &HashMap<String, Miss> {
        &self.entries
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// Mutate one entry in place (verify flow). Marks the stash dirty but
    /// does NOT write - batch the updates, then [`save`](Self::save) once.
    pub fn update(&mut self, key: &str, f: impl FnOnce(&mut Miss)) {
        if let Some(m) = self.entries.get_mut(key) {
            f(m);
            self.dirty = true;
        }
    }

    /// Persist pending updates (atomic, same path as the daemon's writes).
    pub fn save(&mut self) {
        self.persist();
    }

    fn entry_key(store: &str, codename: Option<&str>, fallback_key: &str) -> String {
        match codename {
            Some(code) if !code.is_empty() => format!("{store}:{code}"),
            _ => fallback_key.to_string(),
        }
    }

    fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        // The other writer may have written since we loaded - adopt its half
        // before flattening the map onto disk.
        self.merge_from_disk();
        let Some(path) = &self.path else { return };
        let Ok(json) = serde_json::to_string_pretty(&self.entries) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Atomic: a half-written stash must never eat accumulated knowledge.
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Adopt the other writer's half of every entry from the current file.
    ///
    /// The daemon owns resolutions, the setup tool owns annotations; each
    /// takes the *other* half from disk (where the other writer put it) and
    /// keeps its own from memory. Entries only the disk knows are kept
    /// whole - nobody ever deletes a miss. This shrinks the lost-update
    /// window from session-long to the read-write gap; the writers are a
    /// human-run CLI and a rare launch event, which do not race in practice.
    fn merge_from_disk(&mut self) {
        let Some(path) = &self.path else { return };
        let Ok(raw) = std::fs::read_to_string(path) else {
            return; // no file yet - nothing to adopt
        };
        let Ok(disk) = serde_json::from_str::<HashMap<String, Miss>>(&raw) else {
            return; // unreadable file - memory is the best surviving copy
        };
        for (key, theirs) in disk {
            match self.entries.get_mut(&key) {
                None => {
                    self.entries.insert(key, theirs);
                }
                Some(ours) if self.annotator => {
                    // The daemon may have refreshed the resolution half.
                    ours.title = theirs.title;
                    ours.store = theirs.store;
                    ours.codename = theirs.codename;
                    ours.umu_id = theirs.umu_id;
                    ours.title_source = theirs.title_source;
                    ours.confidence = theirs.confidence;
                    ours.executable = theirs.executable;
                    ours.first_seen = theirs.first_seen;
                    ours.last_seen = theirs.last_seen;
                    ours.launcher = theirs.launcher;
                    ours.launcher_name = theirs.launcher_name;
                    ours.launcher_dir = theirs.launcher_dir;
                    ours.codename_source = theirs.codename_source;
                    ours.runner = theirs.runner;
                }
                Some(ours) => {
                    // The setup tool may have annotated since we loaded.
                    ours.verification = theirs.verification;
                    ours.drafted_id = theirs.drafted_id;
                    ours.possible_pr = theirs.possible_pr;
                    ours.fix = theirs.fix;
                    ours.store_override = theirs.store_override;
                    ours.codename_override = theirs.codename_override;
                    ours.codename_override_source = theirs.codename_override_source;
                    ours.title_override = theirs.title_override;
                    ours.dismissed = theirs.dismissed;
                    ours.umu_promoted = theirs.umu_promoted;
                }
            }
        }
    }
}

/// Adopt a fact when this launch carries one. An absent or empty value
/// leaves whatever is already there: launcher facts accumulate, they never
/// blank out.
fn fill(slot: &mut Option<String>, value: Option<String>) {
    if let Some(v) = value {
        if !v.is_empty() {
            *slot = Some(v);
        }
    }
}

fn rank(c: Confidence) -> u8 {
    match c {
        Confidence::Low => 1,
        Confidence::Medium => 2,
        Confidence::High => 3,
    }
}

/// Today as `YYYY-MM-DD`, computed without a chrono dependency.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil-date conversion (days → y/m/d), Howard Hinnant's algorithm.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Translate a launcher's own store id into the umu-database's spelling.
///
/// Lutris names its services in its own vocabulary (`ea_app`,
/// `humblebundle`, `steamwindows`) where the database says `ea`, `humble`,
/// `steam`; the spellings the two already share pass straight through.
/// Anything that names no store - `flathub`, empty, `unknown`, a service we
/// have never seen - is `none`, the database's own word for "standalone".
/// Shared by the daemon's guess and the setup tool so one string never means
/// two stores.
pub fn normalize_store(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        // Launcher spellings that need translating.
        "ea_app" => "ea",
        "humblebundle" => "humble",
        "steamwindows" => "steam",
        // Spellings the umu-database already uses, passed through.
        "steam" => "steam",
        "gog" => "gog",
        "egs" => "egs",
        "ubisoft" => "ubisoft",
        "zoomplatform" => "zoomplatform",
        "humble" => "humble",
        "itchio" => "itchio",
        "amazon" => "amazon",
        "battlenet" => "battlenet",
        "ea" => "ea",
        "umu" => "umu",
        // `flathub` is a distribution channel, not a store; empty and
        // `unknown` say nothing; anything else is unproven.
        _ => "none",
    }
}

/// Map launch evidence to a umu-database store id. An explicit store from
/// the launcher settles it; failing that `HEROIC_APP_SOURCE` speaks for
/// Heroic launches, and for the rest the install path sometimes does.
/// Anything unproven is `none` - the label is a guess and says so.
pub fn guess_store(
    explicit: Option<&str>,
    heroic_source: Option<&str>,
    executable: &str,
) -> String {
    // A launcher that names the store outranks every sniff below. `none`
    // is not an answer, so an unrecognised name falls through rather than
    // shutting the guessing down.
    if let Some(raw) = explicit {
        let normalized = normalize_store(raw);
        if normalized != "none" {
            return normalized.to_string();
        }
    }
    match heroic_source {
        Some("epic") => return "egs".to_string(),
        Some("gog") => return "gog".to_string(),
        Some("amazon" | "nile") => return "amazon".to_string(),
        _ => {}
    }
    let exe = executable.to_lowercase();
    if exe.contains("ubisoft") {
        "ubisoft".to_string()
    } else if exe.contains("gog galaxy") || exe.contains("gog games") {
        "gog".to_string()
    } else {
        "none".to_string()
    }
}

/// One row of the umu-database, whichever shape it arrived in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UmuEntry {
    pub title: String,
    pub store: String,
    pub codename: String,
    pub umu_id: String,
}

/// The upstream database, parsed and indexed. Sources, in the order
/// the setup tool tries them: a git checkout's CSV (`--db`/`GAMEBUS_UMU_DB`),
/// the cached API full dump, or nothing - verification then degrades to
/// per-entry API queries. Both shapes parse here; both are real, captured
/// samples in the tests below.
#[derive(Debug, Default)]
pub struct UmuDb {
    entries: Vec<UmuEntry>,
    /// `(store, codename)` lowercased → entry index. Codenames `none`/empty
    /// are not indexed: every standalone row shares them, a hit would be
    /// meaningless.
    by_store_codename: HashMap<(String, String), usize>,
    /// lowercased title → entry indexes (one game, many stores).
    by_title: HashMap<String, Vec<usize>>,
    /// lowercased umu id → entry indexes - the collision index.
    by_umu_id: HashMap<String, Vec<usize>>,
}

impl UmuDb {
    /// Parse either shape: the repo CSV (header `TITLE,STORE,CODENAME,
    /// UMU_ID,…`) or the API's full JSON dump (`[{"title":…,"umu_id":…,
    /// "codename":…,"store":…},…]`). Detected by the first byte.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let entries = if raw.trim_start().starts_with('[') {
            Self::parse_json(raw)?
        } else {
            Self::parse_csv(raw)?
        };
        if entries.is_empty() {
            return Err("database parsed to zero entries".into());
        }
        Ok(Self::index(entries))
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::parse(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Where `--fetch` caches the API full dump.
    pub fn cache_path() -> Option<PathBuf> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(cache.join("gamebus-presenced").join("umu-database.json"))
    }

    fn parse_json(raw: &str) -> Result<Vec<UmuEntry>, String> {
        #[derive(Deserialize)]
        struct Row {
            title: Option<String>,
            umu_id: Option<String>,
            codename: Option<String>,
            store: Option<String>,
        }
        let rows: Vec<Row> =
            serde_json::from_str(raw).map_err(|e| format!("not the API JSON dump: {e}"))?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                Some(UmuEntry {
                    title: r.title.filter(|t| !t.is_empty())?,
                    umu_id: r.umu_id.filter(|i| !i.is_empty())?,
                    store: r.store.unwrap_or_else(|| "none".into()),
                    codename: r.codename.unwrap_or_else(|| "none".into()),
                })
            })
            .collect())
    }

    fn parse_csv(raw: &str) -> Result<Vec<UmuEntry>, String> {
        let mut entries = Vec::new();
        for (i, record) in csv_records(raw).into_iter().enumerate() {
            if i == 0 && record.first().map(String::as_str) == Some("TITLE") {
                continue; // the header row
            }
            // TITLE,STORE,CODENAME,UMU_ID - anything shorter is malformed
            // and skipped; the zero-entries check upstream catches a file
            // that is not a CSV at all.
            if record.len() < 4 || record[0].is_empty() || record[3].is_empty() {
                continue;
            }
            entries.push(UmuEntry {
                title: record[0].clone(),
                store: record[1].clone(),
                codename: record[2].clone(),
                umu_id: record[3].clone(),
            });
        }
        Ok(entries)
    }

    fn index(entries: Vec<UmuEntry>) -> Self {
        let mut db = Self {
            entries,
            ..Self::default()
        };
        for (i, e) in db.entries.iter().enumerate() {
            let code = e.codename.to_lowercase();
            if !code.is_empty() && code != "none" {
                db.by_store_codename
                    .entry((e.store.to_lowercase(), code))
                    .or_insert(i);
            }
            db.by_title
                .entry(e.title.to_lowercase())
                .or_default()
                .push(i);
            db.by_umu_id
                .entry(e.umu_id.to_lowercase())
                .or_default()
                .push(i);
        }
        db
    }

    // parse() rejects empty databases, so an is_empty() could never return
    // true and would be dead in both binaries.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The exact-launch lookup: this store, this codename. A hit means the
    /// launcher missed, not the database.
    pub fn find_store_codename(&self, store: &str, codename: &str) -> Option<&UmuEntry> {
        if codename.is_empty() || codename.eq_ignore_ascii_case("none") {
            return None;
        }
        self.by_store_codename
            .get(&(store.to_lowercase(), codename.to_lowercase()))
            .map(|&i| &self.entries[i])
    }

    /// Every entry sharing this title, case-insensitively - cross-store
    /// candidates.
    pub fn find_title(&self, title: &str) -> Vec<&UmuEntry> {
        self.by_title
            .get(&title.to_lowercase())
            .map(|idx| idx.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    /// Every entry holding this umu id - the collision check.
    pub fn find_umu_id(&self, umu_id: &str) -> Vec<&UmuEntry> {
        self.by_umu_id
            .get(&umu_id.to_lowercase())
            .map(|idx| idx.iter().map(|&i| &self.entries[i]).collect())
            .unwrap_or_default()
    }

    /// Candidates for a human pick: case-insensitive substring match in both
    /// directions — a database title containing the query ("Control" finds
    /// "Control Ultimate Edition") or the query containing a database title
    /// (a decorated launcher title finds the plain row). Ranked exact match,
    /// then database-title-starts-with-query, then the rest; capped at 20.
    /// The upstream CSV carries literal duplicate rows, so results dedup by
    /// (store, codename, umu id). Empty or whitespace queries match nothing.
    pub fn search_title(&self, query: &str) -> Vec<&UmuEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut seen: std::collections::HashSet<(String, String, String)> = Default::default();
        let mut ranked: Vec<(u8, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let t = e.title.to_lowercase();
            let rank = if t == q {
                0
            } else if t.starts_with(&q) {
                1
            } else if t.contains(&q) || q.contains(&t) {
                2
            } else {
                continue;
            };
            if seen.insert((
                e.store.to_lowercase(),
                e.codename.to_lowercase(),
                e.umu_id.to_lowercase(),
            )) {
                ranked.push((rank, i));
            }
        }
        // Stable sort: within a rank, database order stands.
        ranked.sort_by_key(|&(rank, _)| rank);
        ranked
            .into_iter()
            .take(20)
            .map(|(_, i)| &self.entries[i])
            .collect()
    }
}

/// Split CSV text into records, honouring quoted fields (embedded commas,
/// doubled-quote escapes, embedded newlines) - the repo file uses all of
/// these except the last.
fn csv_records(raw: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => in_quotes = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                if record.len() > 1 || !record[0].is_empty() {
                    records.push(std::mem::take(&mut record));
                }
                record.clear();
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        if record.len() > 1 || !record[0].is_empty() {
            records.push(record);
        }
    }
    records
}

/// What drafting an id produced. The collision check is not optional:
/// every branch consults the database's id index before proposing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftOutcome {
    /// A fresh id, absent from the database at check time.
    Drafted { id: String, basis: DraftBasis },
    /// The id already names this very title in the database - no draft
    /// needed, that IS the id to submit (the cross-store sharing rule).
    ExistingId { id: String },
    /// The id already names a *different* game - the draft is discarded and
    /// the conflict is for the NOTE column, not the id one.
    Collision { id: String, existing_title: String },
    /// Nothing safe to draft from: no Steam sku, no lettered codename, and
    /// a title with no letters either.
    NoBasis,
}

/// Draft a umu id per the database's own rules, then collision-check it.
///
/// The ladder, strongest basis first:
/// 1. A Steam sku for the title (from detectable.json) → `umu-<appid>` -
///    the id the database itself would assign.
/// 2. A codename that doubles as a store product id AND contains a letter →
///    `umu-<codename>`. The letter is load-bearing: Proton parses a numeric
///    second part as a SteamAppId, so pure-numeric ids (GOG's, notably)
///    must never be drafted.
/// 3. The standalone rule: `umu-<title-slug>`, lowercase alphanumerics in
///    the database's own house style (`umu-richardburnsrally`).
pub fn draft_umu_id(
    db: &UmuDb,
    title: &str,
    codename: Option<&str>,
    steam_appid: Option<&str>,
) -> DraftOutcome {
    let (id, basis) = if let Some(appid) = steam_appid.filter(|a| valid_drafting_appid(a)) {
        (format!("umu-{appid}"), DraftBasis::SteamSku)
    } else if let Some(code) = codename.filter(|c| codename_is_store_id(c)) {
        (format!("umu-{}", code.to_lowercase()), DraftBasis::StoreId)
    } else {
        let slug = slugify(title);
        if !slug.chars().any(|c| c.is_ascii_alphabetic()) {
            return DraftOutcome::NoBasis;
        }
        (format!("umu-{slug}"), DraftBasis::TitleSlug)
    };
    let holders = db.find_umu_id(&id);
    if holders.is_empty() {
        DraftOutcome::Drafted { id, basis }
    } else if holders.iter().any(|e| e.title.eq_ignore_ascii_case(title)) {
        DraftOutcome::ExistingId { id }
    } else {
        DraftOutcome::Collision {
            id,
            existing_title: holders[0].title.clone(),
        }
    }
}

/// An appid worth drafting from: digits only, non-zero.
fn valid_drafting_appid(appid: &str) -> bool {
    !appid.is_empty()
        && appid.chars().all(|c| c.is_ascii_digit())
        && appid.chars().any(|c| c != '0')
}

/// Does this codename work as a store product id in a umu id? It must carry
/// at least one letter (the Proton numeric-is-SteamAppId rule) and look like
/// an identifier, not free text (EGS app names and store GUIDs qualify).
fn codename_is_store_id(code: &str) -> bool {
    !code.is_empty()
        && !code.eq_ignore_ascii_case("none")
        && code.chars().any(|c| c.is_ascii_alphabetic())
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Lowercased alphanumerics of the title - the database's made-up-id style.
fn slugify(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> UmuReport {
        UmuReport::default() // no path: persist() no-ops, pure in-memory
    }

    #[test]
    fn launch_then_title_builds_a_submission_shaped_entry() {
        let mut r = report();
        r.note_launch("egs", Some("Calluna"), "umu-0", "heroic:Calluna");
        r.note_title(
            "egs",
            Some("Calluna"),
            "heroic:Calluna",
            "Control",
            "heroic-config",
            Confidence::High,
            Some("Control_DX12.exe"),
        );
        let e = r.entries.get("egs:Calluna").expect("entry exists");
        assert_eq!(e.title.as_deref(), Some("Control"));
        assert_eq!(e.store, "egs");
        assert_eq!(e.umu_id, "umu-0");
        assert_eq!(e.confidence, Some(Confidence::High));
        assert_eq!(e.executable.as_deref(), Some("Control_DX12.exe"));
    }

    #[test]
    fn weaker_titles_never_overwrite_stronger_ones() {
        let mut r = report();
        r.note_launch("none", None, "umu-default", "lutris:abc");
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "Amnesia: The Bunker",
            "lutris-wrapper",
            Confidence::Medium,
            None,
        );
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "amnesia",
            "stem",
            Confidence::Low,
            None,
        );
        let e = r.entries.get("lutris:abc").unwrap();
        assert_eq!(e.title.as_deref(), Some("Amnesia: The Bunker"));
        assert_eq!(e.confidence, Some(Confidence::Medium));
        // A stronger source upgrades.
        r.note_title(
            "none",
            None,
            "lutris:abc",
            "Amnesia: The Bunker",
            "detectable",
            Confidence::High,
            Some("AmnesiaTheBunker.exe"),
        );
        let e = r.entries.get("lutris:abc").unwrap();
        assert_eq!(e.confidence, Some(Confidence::High));
    }

    #[test]
    fn a_title_without_a_launch_records_nothing() {
        let mut r = report();
        r.note_title(
            "egs",
            Some("X"),
            "heroic:X",
            "Ghost",
            "stem",
            Confidence::Low,
            None,
        );
        assert!(r.entries.is_empty());
    }

    #[test]
    fn store_guessing_is_conservative() {
        assert_eq!(guess_store(None, Some("epic"), ""), "egs");
        assert_eq!(guess_store(None, Some("gog"), ""), "gog");
        assert_eq!(
            guess_store(
                None,
                None,
                "/games/ubisoft/drive_c/Program Files (x86)/Ubisoft/x.exe"
            ),
            "ubisoft"
        );
        assert_eq!(guess_store(None, None, "/games/somewhere/game.exe"), "none");
    }

    #[test]
    fn an_explicit_launcher_store_outranks_every_sniff() {
        // Lutris' own spelling, translated, beating a contradicting path.
        assert_eq!(
            guess_store(Some("ea_app"), None, "/games/ubisoft/x.exe"),
            "ea"
        );
        assert_eq!(guess_store(Some("humblebundle"), None, ""), "humble");
        // A store string that names nothing falls through to the old
        // evidence rather than shutting the guessing down.
        assert_eq!(guess_store(Some("flathub"), Some("epic"), ""), "egs");
        assert_eq!(guess_store(Some(""), Some("gog"), ""), "gog");
        assert_eq!(
            guess_store(Some("unknown"), None, "/games/ubisoft/x.exe"),
            "ubisoft"
        );
    }

    #[test]
    fn store_names_normalise_to_the_databases_spelling() {
        for (raw, want) in [
            // Launcher spellings that need translating.
            ("ea_app", "ea"),
            ("humblebundle", "humble"),
            ("steamwindows", "steam"),
            // Spellings the database already uses, passed through.
            ("steam", "steam"),
            ("gog", "gog"),
            ("egs", "egs"),
            ("ubisoft", "ubisoft"),
            ("zoomplatform", "zoomplatform"),
            ("humble", "humble"),
            ("itchio", "itchio"),
            ("amazon", "amazon"),
            ("battlenet", "battlenet"),
            ("ea", "ea"),
            ("umu", "umu"),
            // Nothing a store: a channel, silence, an admission, a stranger.
            ("flathub", "none"),
            ("", "none"),
            ("unknown", "none"),
            ("none", "none"),
            ("some-new-launcher", "none"),
            // Case and stray whitespace never change the answer.
            ("EA_App", "ea"),
            ("  SteamWindows  ", "steam"),
        ] {
            assert_eq!(normalize_store(raw), want, "normalize_store({raw:?})");
        }
    }

    #[test]
    fn today_is_a_plausible_iso_date() {
        let d = today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"), "{d}");
    }

    /// A throwaway stash path for the two-writer tests.
    struct TempStash(PathBuf);
    impl TempStash {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "gamebus-umu-report-test-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.join("umu-misses.json"))
        }
    }
    impl Drop for TempStash {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    }

    #[test]
    fn the_two_writers_never_clobber_each_others_half() {
        let stash = TempStash::new("two-writers");

        // The daemon records a miss and keeps running (stale in-memory copy).
        let mut daemon = UmuReport::from_path(stash.0.clone());
        daemon.note_launch("egs", Some("Calluna"), "umu-0", "heroic:Calluna");

        // The setup tool verifies: loads, annotates, saves.
        let mut setup = UmuReport {
            annotator: true,
            ..UmuReport::from_path(stash.0.clone())
        };
        setup.update("egs:Calluna", |m| {
            m.verification = Some(Verification {
                state: VerificationState::ConfirmedMissing,
                umu_id: None,
                checked: "2026-08-07".into(),
                note: None,
            });
            m.drafted_id = Some(DraftedId {
                id: "umu-870780".into(),
                basis: DraftBasis::SteamSku,
                collision_checked: "2026-08-07".into(),
            });
            m.codename_override = Some("Calluna".into());
            m.title_override = Some("Control Ultimate Edition".into());
        });
        setup.save();

        // The game launches again: the daemon persists from its PRE-verify
        // copy, this time with the launcher facts it could read. The
        // annotations must survive.
        daemon.note_launch_with(
            "egs",
            Some("Calluna"),
            "umu-0",
            "heroic:Calluna",
            LaunchFacts {
                launcher: Some("heroic".into()),
                launcher_name: Some("Control".into()),
                launcher_dir: Some("/games/Control".into()),
                codename_source: Some("heroic-env".into()),
                runner: Some("proton".into()),
            },
        );
        // And the daemon records a brand-new miss the setup tool never saw.
        daemon.note_launch("gog", Some("1207600000"), "umu-0", "heroic:1207600000");

        // The setup tool saves again from ITS stale copy: the new miss must
        // survive too.
        setup.update("egs:Calluna", |m| m.possible_pr = None);
        setup.save();

        let disk: HashMap<String, Miss> =
            serde_json::from_str(&std::fs::read_to_string(&stash.0).unwrap()).unwrap();
        let calluna = &disk["egs:Calluna"];
        assert!(
            calluna.verification.is_some(),
            "daemon write erased the verification"
        );
        assert_eq!(
            calluna.drafted_id.as_ref().map(|d| d.id.as_str()),
            Some("umu-870780"),
            "daemon write erased the drafted id"
        );
        assert_eq!(
            calluna.codename_override.as_deref(),
            Some("Calluna"),
            "daemon write erased the codename override"
        );
        assert_eq!(
            calluna.title_override.as_deref(),
            Some("Control Ultimate Edition"),
            "daemon write erased the title override"
        );
        assert!(
            disk.contains_key("gog:1207600000"),
            "setup save erased the daemon's new miss"
        );
        // The launcher facts are resolution-half: the annotator's save
        // adopted them from disk instead of writing its stale Nones back.
        assert_eq!(calluna.launcher.as_deref(), Some("heroic"));
        assert_eq!(calluna.launcher_name.as_deref(), Some("Control"));
        assert_eq!(calluna.launcher_dir.as_deref(), Some("/games/Control"));
        assert_eq!(calluna.codename_source.as_deref(), Some("heroic-env"));
        assert_eq!(calluna.runner.as_deref(), Some("proton"));
    }

    #[test]
    fn launcher_facts_are_set_once_and_never_blanked() {
        let mut r = report();
        r.note_launch_with(
            "none",
            None,
            "",
            "lutris:1234",
            LaunchFacts {
                launcher: Some("lutris".into()),
                launcher_name: Some("Amnesia: The Bunker".into()),
                launcher_dir: Some("/games/amnesia".into()),
                codename_source: Some("lutris-config".into()),
                runner: Some("proton".into()),
            },
        );
        let e = r.entries.get("lutris:1234").expect("entry exists");
        assert_eq!(e.launcher.as_deref(), Some("lutris"));
        assert_eq!(e.launcher_name.as_deref(), Some("Amnesia: The Bunker"));
        assert_eq!(e.launcher_dir.as_deref(), Some("/games/amnesia"));
        assert_eq!(e.codename_source.as_deref(), Some("lutris-config"));
        assert_eq!(e.runner.as_deref(), Some("proton"));
        let first_seen = e.first_seen.clone();

        // A later, quieter launch (and the plain note_launch, which carries
        // no facts at all) must not blank a single one of them.
        r.note_launch_with(
            "none",
            None,
            "",
            "lutris:1234",
            LaunchFacts {
                launcher_name: Some(String::new()),
                runner: Some("native".into()),
                ..LaunchFacts::default()
            },
        );
        r.note_launch("none", None, "", "lutris:1234");
        let e = r.entries.get("lutris:1234").unwrap();
        assert_eq!(e.launcher.as_deref(), Some("lutris"));
        assert_eq!(
            e.launcher_name.as_deref(),
            Some("Amnesia: The Bunker"),
            "an empty fact blanked one that was already known"
        );
        assert_eq!(e.launcher_dir.as_deref(), Some("/games/amnesia"));
        assert_eq!(e.codename_source.as_deref(), Some("lutris-config"));
        // A fact that DID arrive refreshes: the game runs native now.
        assert_eq!(e.runner.as_deref(), Some("native"));
        assert_eq!(e.first_seen, first_seen);
    }

    #[test]
    fn a_promotion_survives_the_daemons_next_persist() {
        let stash = TempStash::new("promotion");
        let mut daemon = UmuReport::from_path(stash.0.clone());
        daemon.note_launch("itchio", Some("926077"), "", "lutris:danger-scavenger");

        let mut setup = UmuReport {
            annotator: true,
            ..UmuReport::from_path(stash.0.clone())
        };
        setup.update("itchio:926077", |m| {
            m.umu_promoted = Some("2026-08-24".into());
        });
        setup.save();

        // The daemon persists from its pre-promotion copy: the merge must
        // adopt the flag like every other annotation.
        daemon.note_launch("itchio", Some("926077"), "", "lutris:danger-scavenger");
        let fresh = UmuReport::from_path(stash.0.clone());
        assert_eq!(
            fresh.entries()["itchio:926077"].umu_promoted.as_deref(),
            Some("2026-08-24"),
            "the promotion flag must survive the daemon's write"
        );
    }

    #[test]
    fn a_launch_without_a_umu_id_is_stashed_but_is_not_a_umu_miss() {
        // The widened shape: a launcher handed us a launch umu never saw.
        let raw = r#"{"lutris:1234":{"title":"Amnesia: The Bunker","store":"none",
            "codename":null,"umu_id":"","title_source":"lutris-wrapper",
            "confidence":"medium","executable":null,
            "first_seen":"2026-08-23","last_seen":"2026-08-23",
            "launcher":"lutris","runner":"native"},
            "egs:Calluna":{"title":"Control","store":"egs","codename":"Calluna",
            "umu_id":"umu-0","title_source":"heroic-config","confidence":"high",
            "executable":null,"first_seen":"2026-08-06","last_seen":"2026-08-06"}}"#;
        let entries: HashMap<String, Miss> = serde_json::from_str(raw).unwrap();
        assert!(!entries["lutris:1234"].is_umu_miss());
        assert!(entries["egs:Calluna"].is_umu_miss());
        // `lutris-wrapper` is a title source like any other - nothing rejects
        // it on the way in or back out.
        assert_eq!(
            entries["lutris:1234"].title_source.as_deref(),
            Some("lutris-wrapper")
        );

        // Round-trip: the empty umu id is ALWAYS written, so a binary from
        // before the widening (which has no default for the field) can still
        // parse what this one wrote.
        let json = serde_json::to_string(&entries).unwrap();
        assert!(json.contains(r#""umu_id":"""#), "{json}");
        let back: HashMap<String, Miss> = serde_json::from_str(&json).unwrap();
        assert!(!back["lutris:1234"].is_umu_miss());
        assert_eq!(back["lutris:1234"].launcher.as_deref(), Some("lutris"));
        assert_eq!(back["lutris:1234"].runner.as_deref(), Some("native"));
        // Facts nobody established stay out of the file entirely.
        assert!(!json.contains("launcher_dir"), "{json}");
    }

    #[test]
    fn a_corrupt_stash_is_reported_and_never_overwritten() {
        let stash = TempStash::new("corrupt");
        std::fs::write(&stash.0, "{\"key\": {\"broken\": true},}").unwrap();
        let before = std::fs::read(&stash.0).unwrap();

        let mut r = UmuReport::from_path(stash.0.clone());
        assert!(r.load_error().is_some(), "parse failure went unreported");
        assert!(r.entries.is_empty());

        // Writes must be refused: the corrupt file holds the only copy of
        // whatever knowledge it still contains.
        r.note_launch("egs", Some("X"), "umu-0", "heroic:X");
        assert_eq!(
            before,
            std::fs::read(&stash.0).unwrap(),
            "a write flattened the corrupt stash"
        );
    }

    #[test]
    fn an_old_stash_still_loads() {
        // Captured from a stash written before the verification fields
        // existed - must deserialize with the new fields defaulting to None.
        let old = r#"{"egs:Calluna":{"title":"Control","store":"egs",
            "codename":"Calluna","umu_id":"umu-0","title_source":"heroic-config",
            "confidence":"high","executable":"Control_DX12.exe",
            "first_seen":"2026-08-06","last_seen":"2026-08-06"}}"#;
        let entries: HashMap<String, Miss> = serde_json::from_str(old).unwrap();
        let m = &entries["egs:Calluna"];
        assert_eq!(m.title.as_deref(), Some("Control"));
        assert!(m.verification.is_none());
        assert!(m.drafted_id.is_none());
        assert!(m.possible_pr.is_none());
        assert!(m.codename_override.is_none());
        assert!(m.title_override.is_none());
        // …and the launcher facts, added when the stash widened past umu.
        assert!(m.launcher.is_none());
        assert!(m.launcher_name.is_none());
        assert!(m.launcher_dir.is_none());
        assert!(m.codename_source.is_none());
        assert!(m.runner.is_none());
        assert!(m.is_umu_miss(), "a umu-0 entry is still a umu miss");
        // Without overrides the effective values are the daemon's own.
        assert_eq!(m.effective_codename(), Some("Calluna"));
        assert_eq!(m.effective_title(), Some("Control"));
    }

    // ---- UmuDb parsing - every sample below is captured, not made up.

    /// Real rows from umu-database.csv: header, plain rows, a quoted NOTE
    /// with commas, and a quoted TITLE with a comma.
    const REAL_CSV: &str = concat!(
        "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
        "Age of Wonders,gog,1207658883,umu-61500,aow,,\n",
        "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
        "Dark and Darker,none,none,umu-2016590,dad,Standalone Dark and Darker installer,\n",
        "Trackmania,ubisoft,5595,umu-2225070,tm,\"also known as 'Trackmania (2020)', not to be confused with 'TrackMania (2003)'\",\n",
        "\"Warhammer 40,000: Space Marine\",gog,1668484481,umu-55150,,,\n",
    );

    /// The API full dump, same games (nulls and all).
    const REAL_JSON: &str = r#"[
        {"title":"Age of Wonders","umu_id":"umu-61500","acronym":"aow","codename":"1207658883","store":"gog","exe_string":null,"notes":null},
        {"title":"Borderlands 3","umu_id":"umu-397540","acronym":"bl3","codename":"Catnip","store":"egs","exe_string":null,"notes":null},
        {"title":"Dark and Darker","umu_id":"umu-2016590","acronym":"dad","codename":"none","store":"none","exe_string":null,"notes":"Standalone Dark and Darker installer"}
    ]"#;

    #[test]
    fn the_repo_csv_parses_with_quotes_and_header() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert_eq!(db.len(), 5);
        let bl3 = db.find_store_codename("egs", "Catnip").unwrap();
        assert_eq!(bl3.umu_id, "umu-397540");
        assert_eq!(bl3.title, "Borderlands 3");
        // The quoted comma-in-title row survived intact.
        let wh = db.find_title("warhammer 40,000: space marine");
        assert_eq!(wh.len(), 1);
        assert_eq!(wh[0].umu_id, "umu-55150");
        // The quoted NOTE did not shift the columns.
        let tm = db.find_store_codename("ubisoft", "5595").unwrap();
        assert_eq!(tm.umu_id, "umu-2225070");
    }

    #[test]
    fn the_api_dump_parses_and_agrees_with_the_csv() {
        let db = UmuDb::parse(REAL_JSON).unwrap();
        assert_eq!(db.len(), 3);
        assert_eq!(
            db.find_store_codename("egs", "Catnip").map(|e| &e.umu_id),
            db.find_title("Borderlands 3").first().map(|e| &e.umu_id),
        );
    }

    #[test]
    fn lookups_are_case_insensitive_where_it_matters() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert!(db.find_store_codename("EGS", "catnip").is_some());
        assert_eq!(db.find_title("BORDERLANDS 3").len(), 1);
        assert_eq!(db.find_umu_id("UMU-397540").len(), 1);
    }

    #[test]
    fn none_codenames_never_hit() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        // Standalone rows all share store/codename "none" - a lookup with
        // those must not "find" Dark and Darker.
        assert!(db.find_store_codename("none", "none").is_none());
        assert!(db.find_store_codename("egs", "").is_none());
    }

    // ---- Title search for the TUI's pick verb (S9b).

    /// Real row shapes: one game in three spellings, plus the CSV's literal
    /// duplicate-row habit (the upstream file really contains repeated rows).
    const SEARCH_CSV: &str = concat!(
        "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
        "Control,egs,Calluna,umu-870780,,,\n",
        "Control Ultimate Edition,gog,2049187585,umu-870780,,,\n",
        "Ground Control,gog,1207658883,umu-groundcontrol,,,\n",
        "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
        "Borderlands 3,egs,Catnip,umu-397540,bl3,,\n",
    );

    #[test]
    fn search_ranks_exact_then_prefix_then_the_rest() {
        let db = UmuDb::parse(SEARCH_CSV).unwrap();
        let hits = db.search_title("control");
        let titles: Vec<&str> = hits.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Control", "Control Ultimate Edition", "Ground Control"]
        );
        // Case-insensitive throughout.
        assert_eq!(db.search_title("CONTROL")[0].title, "Control");
    }

    #[test]
    fn search_matches_substrings_in_both_directions() {
        let db = UmuDb::parse(SEARCH_CSV).unwrap();
        // Query inside a database title.
        assert!(db
            .search_title("ultimate")
            .iter()
            .any(|e| e.title == "Control Ultimate Edition"));
        // Database title inside the query — a decorated launcher title still
        // finds the plain row.
        assert!(db
            .search_title("Control Ultimate Edition GOTY")
            .iter()
            .any(|e| e.title == "Control Ultimate Edition"));
        assert!(db.search_title("Half-Life").is_empty());
    }

    #[test]
    fn search_dedups_the_csvs_literal_duplicate_rows() {
        let db = UmuDb::parse(SEARCH_CSV).unwrap();
        let hits = db.search_title("Borderlands 3");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].umu_id, "umu-397540");
    }

    #[test]
    fn search_caps_at_twenty_and_ignores_empty_queries() {
        let mut csv = String::from(
            "TITLE,STORE,CODENAME,UMU_ID,COMMON ACRONYM (Optional),NOTE (Optional),EXE_STRINGS (Optional)\n",
        );
        for i in 0..25 {
            csv.push_str(&format!("Fixture Quest {i:02},egs,code{i},umu-fq{i},,,\n"));
        }
        let db = UmuDb::parse(&csv).unwrap();
        assert_eq!(db.search_title("fixture quest").len(), 20);
        assert!(db.search_title("").is_empty());
        assert!(db.search_title("   ").is_empty());
    }

    #[test]
    fn garbage_input_is_an_error_not_an_empty_database() {
        assert!(UmuDb::parse("").is_err());
        assert!(UmuDb::parse("[]").is_err());
        assert!(UmuDb::parse("[{\"nonsense\":true}]").is_err());
        assert!(UmuDb::parse("not,a,database\n").is_err()); // < 4 columns
        assert!(UmuDb::parse("{\"an\":\"object\"}").is_err()); // JSON, wrong shape
    }

    // ---- Drafting truth table.

    #[test]
    fn a_steam_sku_wins_and_drafts_the_appid() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert_eq!(
            draft_umu_id(&db, "Control", Some("Calluna"), Some("870780")),
            DraftOutcome::Drafted {
                id: "umu-870780".into(),
                basis: DraftBasis::SteamSku
            }
        );
    }

    #[test]
    fn a_lettered_codename_drafts_a_store_id() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert_eq!(
            draft_umu_id(&db, "Some Game", Some("Kumquat"), None),
            DraftOutcome::Drafted {
                id: "umu-kumquat".into(),
                basis: DraftBasis::StoreId
            }
        );
        // GUID-shaped store ids qualify too (they carry letters).
        assert_eq!(
            draft_umu_id(
                &db,
                "I, Hope",
                Some("556a4404-a3f6-4ac4-8e0a-c1b4e9787255"),
                None
            ),
            DraftOutcome::Drafted {
                id: "umu-556a4404-a3f6-4ac4-8e0a-c1b4e9787255".into(),
                basis: DraftBasis::StoreId
            }
        );
    }

    #[test]
    fn numeric_codenames_fall_through_to_the_slug() {
        // A GOG-style pure-numeric codename would be parsed as a SteamAppId
        // by Proton - it must never become the id.
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert_eq!(
            draft_umu_id(&db, "Jazzpunk!", Some("1207658883999"), None),
            DraftOutcome::Drafted {
                id: "umu-jazzpunk".into(),
                basis: DraftBasis::TitleSlug
            }
        );
    }

    #[test]
    fn slugs_always_carry_a_letter_or_nothing_is_drafted() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        assert_eq!(draft_umu_id(&db, "1979", None, None), DraftOutcome::NoBasis);
        assert_eq!(draft_umu_id(&db, "", None, None), DraftOutcome::NoBasis);
    }

    #[test]
    fn a_collision_with_the_same_title_is_the_cross_store_id() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        // Borderlands 3 from another store, appid 397540: the id exists AND
        // names the same game - use it, do not re-draft.
        assert_eq!(
            draft_umu_id(&db, "Borderlands 3", Some("SomeCode"), Some("397540")),
            DraftOutcome::ExistingId {
                id: "umu-397540".into()
            }
        );
    }

    #[test]
    fn a_collision_with_a_different_title_discards_the_draft() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        // A (hypothetical) wrong appid pointing at an existing entry of a
        // different game: the draft dies, the conflict is reported.
        assert_eq!(
            draft_umu_id(&db, "Not Borderlands", None, Some("397540")),
            DraftOutcome::Collision {
                id: "umu-397540".into(),
                existing_title: "Borderlands 3".into()
            }
        );
    }

    #[test]
    fn zero_and_empty_appids_never_draft() {
        let db = UmuDb::parse(REAL_CSV).unwrap();
        // "0" is umu's own miss marker, never a real appid.
        assert_eq!(
            draft_umu_id(&db, "Mystery", None, Some("0")),
            DraftOutcome::Drafted {
                id: "umu-mystery".into(),
                basis: DraftBasis::TitleSlug
            }
        );
        assert_eq!(
            draft_umu_id(&db, "Mystery", None, Some("")),
            DraftOutcome::Drafted {
                id: "umu-mystery".into(),
                basis: DraftBasis::TitleSlug
            }
        );
    }
}
