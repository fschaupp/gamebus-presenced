//! Folding the miss stash into gamebus-gamedb pages.
//!
//! The stash is one entry per *launch identity* - a store and a codename,
//! or a wrapper key when the launcher gave up neither. A gamedb page is one
//! entry per *game*, with every store it appears on nested inside, because
//! "these store identities are the same game" is the claim a reviewer is
//! there to judge. This module is the fold between the two, and it is pure:
//! stash in, page text out, no disk and no network.
//!
//! Everything here follows `gamedb/CONTRIBUTING.md`, which is the binding
//! description of the format; the id precedence in [`Candidate::canonical_id`]
//! is that document's table, in its order.

use std::collections::BTreeMap;

use crate::umu_report::{Confidence, DraftBasis, Miss, UmuReport};

use super::index::GamedbIndex;

/// The stores that can lend a page its canonical id, in the precedence
/// `gamedb/CONTRIBUTING.md` lays down. Steam outranks all of them and is
/// handled separately; `umu` and `steam` never appear here, because a umu
/// id is only ever recorded when umu-database really named the game.
const ID_STORES: [&str; 9] = [
    "gog",
    "egs",
    "ubisoft",
    "ea",
    "battlenet",
    "amazon",
    "humble",
    "itchio",
    "zoomplatform",
];

/// One `[[stores.<store>]]` block: one product, on one store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StoreEntry {
    pub(super) store: String,
    pub(super) codename: String,
    /// The executable as observed for THIS store's copy, basename only.
    pub(super) exe: Option<String>,
    pub(super) seen: String,
    pub(super) source: String,
    pub(super) confidence: String,
}

/// Something this machine knows that the published page does not: one fact,
/// and one edit to that page's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Addition {
    /// A store product the page does not list.
    Store(StoreEntry),
    /// An executable no alias on the page resolves.
    Exe(String),
    /// The Steam app id, for a page whose `games.steam` is null.
    Steam { appid: u64, seen: String },
}

impl Addition {
    /// How the addition reads in a report line: `+gog/1660194629`,
    /// `+exe Control_DX12.exe`, `+steam 870780`.
    pub(super) fn label(&self) -> String {
        match self {
            Addition::Store(entry) => format!("+{}/{}", entry.store, entry.codename),
            Addition::Exe(exe) => format!("+exe {exe}"),
            Addition::Steam { appid, .. } => format!("+steam {appid}"),
        }
    }
}

/// Every addition, in one phrase: `+gog/1660194629, +exe Control_DX12.exe`.
pub(super) fn additions_label(additions: &[Addition]) -> String {
    additions
        .iter()
        .map(Addition::label)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Where a page stands against what gamebus-gamedb already publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Status {
    /// One of the page's own identifiers already resolves upstream, and the
    /// page carries everything this machine knows. Adding a second page for
    /// it is the mistake `CONTRIBUTING.md` opens with.
    InGamedb { id: String, title: String },
    /// The same, except this machine knows something the page does not - a
    /// store the data set never saw, an executable, a Steam app id. Not a
    /// new page: an edit to the one that exists.
    Enhance {
        id: String,
        title: String,
        /// Never empty; an enhancement with nothing to add is `InGamedb`.
        additions: Vec<Addition>,
    },
    /// Nothing upstream claims it, and it identifies itself.
    Ready,
    /// It would not pass the lint, and says why.
    Incomplete { reason: String },
}

/// One game, folded out of every stash entry that turned out to be it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    /// What the fold grouped on. Not written to the page - it exists so the
    /// TUI can keep a selection on the same game across a refresh.
    pub(super) key: String,
    pub(super) title: Option<String>,
    /// The Steam app id, where a `steam-sku` draft proved one.
    pub(super) steam: Option<u64>,
    pub(super) stores: Vec<StoreEntry>,
    /// Executables that identify the game where no store does - basenames,
    /// in the capitalization they were observed with.
    pub(super) exes: Vec<String>,
    pub(super) note: Option<String>,
    /// The newest `last_seen` of everything folded in.
    pub(super) seen: String,
    /// How many stash entries this page speaks for.
    pub(super) entries: usize,
    /// Every stash key folded into this page, sorted - the gamedb pane shows
    /// games, but its correction keys act on stash entries.
    pub(super) entry_keys: Vec<String>,
    /// The one of [`Candidate::entry_keys`] a correction should land on: the
    /// entry carrying a store identity, else the most recently seen. It is
    /// the entry the fold reads first, so correcting it corrects the page.
    pub(super) rep_key: String,
    pub(super) status: Status,
}

impl Candidate {
    /// The canonical id, by `gamedb/CONTRIBUTING.md`'s precedence: the Steam
    /// app id, then a store codename taking stores in the documented order.
    /// A umu id never appears - the stash's drafted ids are proposals to
    /// umu-database, not memberships in it, and writing one would claim a
    /// membership the game does not have. `None` means the page would need
    /// a minted `gamedb-` id, which is a human's decision and not ours.
    pub(super) fn canonical_id(&self) -> Option<String> {
        if let Some(appid) = self.steam {
            return Some(format!("steam-{appid}"));
        }
        for store in ID_STORES {
            if let Some(entry) = self.stores.iter().find(|e| e.store == store) {
                return Some(format!("{}-{}", entry.store, entry.codename));
            }
        }
        None
    }

    /// Every identifier this page would claim, in the same shape the data
    /// set's `aliases` table publishes them. What the index is asked about.
    pub(super) fn identifiers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(appid) = self.steam {
            out.push(format!("steam-{appid}"));
        }
        for entry in &self.stores {
            out.push(format!("{}-{}", entry.store, entry.codename));
            if let Some(exe) = &entry.exe {
                out.push(format!("exe:{}", exe.to_lowercase()));
            }
        }
        for exe in &self.exes {
            out.push(format!("exe:{}", exe.to_lowercase()));
        }
        out
    }

    /// The file this page belongs in, relative to `games/`.
    pub(super) fn file_name(&self) -> String {
        format!("{}.toml", slug(self.title.as_deref().unwrap_or_default()))
    }
}

/// Fold the stash into one candidate page per game.
///
/// The grouping key is the strongest identity the entry carries, first that
/// applies: the Steam app id a `steam-sku` draft established, else the
/// store and codename, else the title. That is what merges eight wrapper
/// launches of Control and its Epic entry into a single page - which is the
/// data set's one-game-one-page rule, applied before the file is written
/// rather than discovered by the lint afterwards.
///
/// Dismissed entries are left out: "not wanted" is a judgement the misses
/// pane already recorded, and it means the same thing here.
pub(super) fn candidates(report: &UmuReport, index: Option<&GamedbIndex>) -> Vec<Candidate> {
    let mut keys: Vec<&String> = report.entries().keys().collect();
    keys.sort();

    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for key in keys {
        let miss = &report.entries()[key];
        if miss.dismissed.is_some() {
            continue;
        }
        let group_key = group_key(key, miss);
        let group = groups.entry(group_key.clone()).or_default();
        if group.key.is_empty() {
            group.key = group_key;
        }
        group.absorb(key, miss);
    }

    groups
        .into_values()
        .map(|group| {
            let mut candidate = group.finish();
            candidate.status = status(&candidate, index);
            candidate
        })
        .collect()
}

/// Which page a stash entry belongs to.
fn group_key(stash_key: &str, miss: &Miss) -> String {
    if let Some(appid) = steam_appid(miss) {
        return format!("steam-{appid}");
    }
    match (miss.effective_store(), miss.effective_codename()) {
        (store, Some(codename)) if store != "none" && !codename.is_empty() => {
            format!("{store}-{codename}")
        }
        // Nothing but a title left to group on. With not even that, the
        // stash key stands in: two unidentified games must not merge into
        // one page just because neither resolved a name.
        _ => match miss.effective_title() {
            Some(title) if !title.trim().is_empty() => title.to_lowercase(),
            _ => format!("stash:{stash_key}"),
        },
    }
}

/// The Steam app id a `steam-sku` draft carries. That basis means the id
/// came from Discord's detectable list, which records the game's own Steam
/// sku - an authority naming the game, which is exactly what the id
/// precedence asks for. Every other basis is our own guess and names
/// nothing.
fn steam_appid(miss: &Miss) -> Option<u64> {
    let drafted = miss.drafted_id.as_ref()?;
    if drafted.basis != DraftBasis::SteamSku {
        return None;
    }
    drafted.id.strip_prefix("umu-")?.parse().ok()
}

/// One page under construction.
#[derive(Debug, Default)]
struct Group {
    key: String,
    title: Option<String>,
    /// Whether the title came from a correction the user typed - which
    /// outranks any resolver's, and must not be overwritten by one.
    title_is_yours: bool,
    steam: Option<u64>,
    stores: BTreeMap<(String, String), StoreEntry>,
    exes: Vec<String>,
    /// The `checked` date of the newest firm "no protonfix upstream".
    no_fix_checked: Option<String>,
    seen: String,
    entries: usize,
    /// Every stash key absorbed, in the order they were walked (sorted).
    entry_keys: Vec<String>,
    /// The representative so far, as `(carries a store identity, last_seen,
    /// key)`. The tuple is the precedence: comparing two picks the winner,
    /// with the key breaking a same-day tie so runs stay deterministic.
    rep: Option<(bool, String, String)>,
}

impl Group {
    fn absorb(&mut self, key: &str, miss: &Miss) {
        self.entries += 1;
        self.entry_keys.push(key.to_string());
        let has_store = matches!(
            (miss.effective_store(), miss.effective_codename()),
            (store, Some(codename)) if store != "none" && !codename.is_empty()
        );
        // Reverse on the key so "smallest" wins a tie, while "largest" wins
        // on the other two fields.
        let rank = (has_store, miss.last_seen.clone(), key.to_string());
        let better = match &self.rep {
            None => true,
            Some((r_store, r_seen, r_key)) => {
                (rank.0, &rank.1, std::cmp::Reverse(&rank.2))
                    > (*r_store, r_seen, std::cmp::Reverse(r_key))
            }
        };
        if better {
            self.rep = Some(rank);
        }
        if miss.last_seen > self.seen {
            self.seen.clone_from(&miss.last_seen);
        }
        let yours = miss.title_override.is_some();
        if let Some(title) = miss.effective_title().filter(|t| !t.trim().is_empty()) {
            if self.title.is_none() || (yours && !self.title_is_yours) {
                self.title = Some(title.to_string());
                self.title_is_yours = yours;
            }
        }
        if self.steam.is_none() {
            self.steam = steam_appid(miss);
        }
        // A firm id with no protonfix upstream is the fact that sends a
        // game here at all: umu-database does not want it, so its store
        // codename has nowhere else to live.
        if let Some(fix) = miss.fix.as_ref().filter(|f| f.fixes.is_empty()) {
            if super::super::umu_misses::id_is_firm(miss)
                && self.no_fix_checked.as_deref() < Some(fix.checked.as_str())
            {
                self.no_fix_checked = Some(fix.checked.clone());
            }
        }

        let exe = miss.executable.as_deref().and_then(game_exe);
        match (miss.effective_store(), miss.effective_codename()) {
            (store, Some(codename)) if store != "none" && !codename.is_empty() => {
                let slot = self
                    .stores
                    .entry((store.to_string(), codename.to_string()))
                    .or_insert_with(|| StoreEntry {
                        store: store.to_string(),
                        codename: codename.to_string(),
                        exe: None,
                        seen: String::new(),
                        source: source_of(miss).to_string(),
                        confidence: confidence_of(miss).to_string(),
                    });
                // The newest observation speaks for the entry; an older one
                // only fills in what the newer never saw.
                if miss.last_seen >= slot.seen {
                    slot.seen.clone_from(&miss.last_seen);
                    slot.source = source_of(miss).to_string();
                    slot.confidence = confidence_of(miss).to_string();
                }
                if slot.exe.is_none() {
                    slot.exe = exe;
                }
            }
            // No store: the executable is all this launch contributed, and
            // it goes to the page's own `exe` list.
            _ => {
                if let Some(exe) = exe {
                    if !self.exes.iter().any(|e| e.eq_ignore_ascii_case(&exe)) {
                        self.exes.push(exe);
                    }
                }
            }
        }
    }

    fn finish(self) -> Candidate {
        let note = self.no_fix_checked.map(|checked| {
            format!(
                "No protonfix upstream (checked {checked}), so umu-database does not \
                 want this game and it has no umu id."
            )
        });
        Candidate {
            key: self.key,
            title: self.title,
            steam: self.steam,
            stores: self.stores.into_values().collect(),
            exes: self.exes,
            note,
            seen: self.seen,
            entries: self.entries,
            rep_key: self.rep.map(|(_, _, key)| key).unwrap_or_default(),
            entry_keys: self.entry_keys,
            status: Status::Ready,
        }
    }
}

/// How a store identity was learned, in the data set's own vocabulary. A
/// correction the user made outranks whatever the resolver reported: they
/// are the source now, and `manual` is the honest word for it.
fn source_of(miss: &Miss) -> &'static str {
    if miss.store_override.is_some() || miss.codename_override.is_some() {
        return "manual";
    }
    match miss.title_source.as_deref() {
        Some("heroic-config") => "heroic-config",
        Some("heroic-library") => "heroic-library",
        Some("detectable") => "detectable",
        // Everything else the daemon can report (a Lutris wrapper argv, an
        // MPRIS hint, an executable stem) has no vocabulary entry, and the
        // schema rejects an invented one. `manual` puts the weight on the
        // note, which is where CONTRIBUTING.md wants it.
        _ => "manual",
    }
}

/// How sure the stash is. An entry nothing resolved has no confidence to
/// report, and `low` is the level that says exactly that.
fn confidence_of(miss: &Miss) -> &'static str {
    match miss.confidence {
        Some(Confidence::High) => "high",
        Some(Confidence::Medium) => "medium",
        _ => "low",
    }
}

/// The basename of a path, whichever slash separates it - the stash records
/// executables as the launcher wrote them, which is sometimes a Windows
/// path.
fn basename(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The executable a page may claim, or nothing. The stash records whatever
/// process the launcher reported, and for a wrapper that is the wrapper:
/// `/usr/bin/python3.13`, `/usr/bin/env`, a launcher script. Those identify
/// no game, and written to a page they would poison the alias table for
/// every game launched the same way. Every umu launch is a Windows game
/// under Proton, so a game's own executable always ends in `.exe`; and a
/// shared helper (a crash handler that ships beside every Unity game) names
/// no game either, which is the whole reason the helper list exists.
fn game_exe(path: &str) -> Option<String> {
    let name = basename(path)?;
    let lower = name.to_lowercase();
    if !lower.ends_with(".exe") || crate::naming::is_shared_helper(&lower) {
        return None;
    }
    Some(name)
}

/// Where a candidate stands: already published (with or without something
/// to add to it), ready to write, or missing what the lint would demand.
fn status(candidate: &Candidate, index: Option<&GamedbIndex>) -> Status {
    if let Some(index) = index {
        let hit = candidate.identifiers().into_iter().find_map(|identifier| {
            index
                .resolve(&identifier)
                .map(|(id, title)| (id.to_string(), title.to_string()))
        });
        if let Some((id, title)) = hit {
            let additions = additions(candidate, index, &id);
            return if additions.is_empty() {
                Status::InGamedb { id, title }
            } else {
                Status::Enhance {
                    id,
                    title,
                    additions,
                }
            };
        }
    }
    if candidate
        .title
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        return Status::Incomplete {
            reason: "no title resolved - correct it in the umu misses tab (t) first".to_string(),
        };
    }
    if candidate.canonical_id().is_none() && candidate.exes.is_empty() {
        return Status::Incomplete {
            reason: "nothing identifies it: no store codename, no Steam appid, no executable"
                .to_string(),
        };
    }
    Status::Ready
}

/// What this machine knows about `id` that the published page does not.
///
/// Every check is "the index resolves this to nobody", never "to somebody
/// else". An identifier another page claims must never be added here:
/// two pages claiming one identifier is the failure
/// `gamedb/CONTRIBUTING.md` spends its longest section on.
fn additions(candidate: &Candidate, index: &GamedbIndex, id: &str) -> Vec<Addition> {
    let mut out = Vec::new();
    // Executables a new store entry carries in its own `exe` field: already
    // written, so they must not also land in the page-level list.
    let mut carried: Vec<String> = Vec::new();

    for entry in &candidate.stores {
        let alias = format!("{}-{}", entry.store, entry.codename);
        if index.has_store(id, &entry.store, &entry.codename) || index.resolve(&alias).is_some() {
            continue;
        }
        if let Some(exe) = &entry.exe {
            carried.push(exe.to_lowercase());
        }
        out.push(Addition::Store(entry.clone()));
    }

    let mut written: Vec<String> = Vec::new();
    let store_exes = candidate.stores.iter().filter_map(|e| e.exe.as_ref());
    for exe in candidate.exes.iter().chain(store_exes) {
        let lower = exe.to_lowercase();
        if carried.contains(&lower)
            || written.contains(&lower)
            || index.resolve(&format!("exe:{lower}")).is_some()
        {
            continue;
        }
        written.push(lower);
        out.push(Addition::Exe(exe.clone()));
    }

    if let Some(appid) = candidate.steam {
        if index.steam(id).is_none() {
            out.push(Addition::Steam {
                appid,
                seen: candidate.seen.clone(),
            });
        }
    }
    out
}

/// The file name a title belongs under: its lowercase slug, exactly as
/// `gamedb/CONTRIBUTING.md` describes it.
pub(super) fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// The page, as TOML text in the layout `gamedb/games/control.toml` uses.
///
/// Written by hand rather than serialized: the daemon's dependency tree is
/// the thing this whole feature must not touch, and a TOML crate for one
/// emitter would be a permanent addition to it. The field order, the `"""`
/// note and the line continuations are the data set's house style, and a
/// golden test pins them.
///
/// One deliberate departure from the order in the format's own examples:
/// the page-level `exe` list is written BEFORE `[ids]`, not after. A bare
/// key after a table header belongs to that table in TOML, so `exe` written
/// underneath `[ids]` would parse as `ids.exe` and the schema would reject
/// the page.
pub(super) fn render_page(candidate: &Candidate) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "title = {}\n",
        toml_string(candidate.title.as_deref().unwrap_or_default())
    ));
    if let Some(id) = candidate.canonical_id() {
        out.push_str(&format!("gamedb = {}\n", toml_string(&id)));
    }
    if let Some(note) = &candidate.note {
        out.push_str(&render_note(note));
    }
    if !candidate.exes.is_empty() {
        let list: Vec<String> = candidate.exes.iter().map(|e| toml_string(e)).collect();
        out.push_str(&format!("exe = [{}]\n", list.join(", ")));
    }
    if let Some(appid) = candidate.steam {
        out.push_str("\n[ids]\n");
        out.push_str(&format!("steam = {appid}\n"));
        out.push_str("source = \"steam-sku\"\n");
        out.push_str(&format!("seen = {}\n", toml_string(&candidate.seen)));
    }
    for entry in &candidate.stores {
        out.push_str(&format!("\n[[stores.{}]]\n", entry.store));
        out.push_str(&format!("codename = {}\n", toml_string(&entry.codename)));
        if let Some(exe) = &entry.exe {
            out.push_str(&format!("exe = {}\n", toml_string(exe)));
        }
        out.push_str(&format!("seen = {}\n", toml_string(&entry.seen)));
        out.push_str(&format!("source = {}\n", toml_string(&entry.source)));
        out.push_str(&format!(
            "confidence = {}\n",
            toml_string(&entry.confidence)
        ));
    }
    out
}

/// The note as a multi-line basic string with line continuations - the
/// shape every page in the data set uses, so a diff between two pages is
/// about the words and not about the wrapping.
fn render_note(note: &str) -> String {
    const WIDTH: usize = 72;
    let mut out = String::from("note = \"\"\"\n");
    let lines = wrap(note, WIDTH);
    for (i, line) in lines.iter().enumerate() {
        out.push_str(&escape(line));
        if i + 1 < lines.len() {
            out.push_str(" \\\n");
        }
    }
    out.push_str("\"\"\"\n");
    out
}

/// Greedy word wrap, like the TUI's - a word longer than the width keeps
/// its own line rather than being broken.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// A TOML basic string. Titles and codenames come from launchers and can
/// hold anything, so nothing here is assumed to be safe.
fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    out.push_str(&escape(value));
    out.push('"');
    out
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::umu_report::{DraftedId, FixCheck};

    /// Build a stash the way the daemon does, then annotate it the way
    /// `--verify` would.

    #[test]
    fn wrapper_processes_and_shared_helpers_never_become_a_page_exe() {
        // The stash records what the launcher reported; for a wrapper that
        // is the wrapper. None of these identifies a game.
        assert_eq!(game_exe("/usr/bin/python3.13"), None);
        assert_eq!(game_exe("/usr/bin/env"), None);
        assert_eq!(game_exe("/home/u/.local/share/Steam/steam.sh"), None);
        assert_eq!(game_exe("C:\\Game\\UnityCrashHandler64.exe"), None);
        // A Windows path, a Unix path, either slash: the game's own exe.
        assert_eq!(
            game_exe("S:\\Spiele\\Call of Duty Black Ops Cold War\\BlackOpsColdWar.exe").as_deref(),
            Some("BlackOpsColdWar.exe")
        );
        assert_eq!(
            game_exe("/media/NVME/Spiele/Project Hospital/ProjectHospital.exe").as_deref(),
            Some("ProjectHospital.exe")
        );
        assert_eq!(
            game_exe("Control_DX12.exe").as_deref(),
            Some("Control_DX12.exe")
        );
    }
    #[derive(Default)]
    struct Stash {
        report: UmuReport,
    }

    impl Stash {
        #[allow(clippy::too_many_arguments)]
        fn launch(
            &mut self,
            store: &str,
            codename: Option<&str>,
            fallback: &str,
            title: Option<&str>,
            source: &str,
            confidence: Confidence,
            exe: Option<&str>,
            seen: &str,
        ) -> String {
            self.report.note_launch(store, codename, "umu-0", fallback);
            if let Some(title) = title {
                self.report
                    .note_title(store, codename, fallback, title, source, confidence, exe);
            }
            let key = match codename {
                Some(c) if !c.is_empty() => format!("{store}:{c}"),
                _ => fallback.to_string(),
            };
            self.report.update(&key, |m| {
                m.first_seen = seen.to_string();
                m.last_seen = seen.to_string();
                if let Some(exe) = exe {
                    m.executable = Some(exe.to_string());
                }
            });
            key
        }

        fn steam_sku(&mut self, key: &str, appid: u64) {
            self.report.update(key, |m| {
                m.drafted_id = Some(DraftedId {
                    id: format!("umu-{appid}"),
                    basis: DraftBasis::SteamSku,
                    collision_checked: "2026-08-22".into(),
                });
            });
        }

        fn no_protonfix(&mut self, key: &str, appid: u64, checked: &str) {
            self.report.update(key, |m| {
                m.fix = Some(FixCheck {
                    umu_id: format!("umu-{appid}"),
                    fixes: Vec::new(),
                    checked: checked.to_string(),
                });
            });
        }
    }

    /// The real Control shape: eight wrapper launches that resolved a title
    /// and an executable but no store, plus the Epic entry that carries the
    /// codename. One game, so one page.
    fn control() -> Stash {
        let mut stash = Stash::default();
        for i in 0..8 {
            let key = stash.launch(
                "none",
                None,
                &format!("wrapper:control-{i}"),
                Some("Control"),
                "detectable",
                Confidence::High,
                Some("Z:\\games\\Control\\Control_DX12.exe"),
                "2026-08-07",
            );
            stash.steam_sku(&key, 870780);
            stash.no_protonfix(&key, 870780, "2026-08-22");
        }
        let key = stash.launch(
            "egs",
            Some("Calluna"),
            "egs:Calluna",
            Some("Control"),
            "heroic-config",
            Confidence::High,
            None,
            "2026-08-15",
        );
        stash.steam_sku(&key, 870780);
        stash
    }

    #[test]
    fn every_launch_of_one_game_folds_into_a_single_page() {
        let stash = control();
        let pages = candidates(&stash.report, None);
        assert_eq!(pages.len(), 1, "{pages:#?}");
        let page = &pages[0];
        assert_eq!(page.key, "steam-870780");
        assert_eq!(page.title.as_deref(), Some("Control"));
        assert_eq!(page.steam, Some(870780));
        assert_eq!(page.entries, 9);
        // The Steam appid outranks the store codename, both for the group
        // key and for the page's own id.
        assert_eq!(page.canonical_id().as_deref(), Some("steam-870780"));
        // One store entry, from the one launch that carried a codename.
        assert_eq!(page.stores.len(), 1);
        assert_eq!(page.stores[0].store, "egs");
        assert_eq!(page.stores[0].codename, "Calluna");
        assert_eq!(page.stores[0].source, "heroic-config");
        assert_eq!(page.stores[0].confidence, "high");
        assert_eq!(page.stores[0].exe, None, "an exe the egs copy never showed");
        // The executable the storeless launches saw, basename only, in the
        // capitalization it was observed with.
        assert_eq!(page.exes, ["Control_DX12.exe"]);
        assert_eq!(page.seen, "2026-08-15");
        assert!(
            page.note
                .as_deref()
                .is_some_and(|n| n.contains("No protonfix upstream (checked 2026-08-22)")),
            "{:?}",
            page.note
        );
        assert_eq!(page.status, Status::Ready);
    }

    /// A game identified by nothing but its executables: no store, and a
    /// Steam appid that only Discord's list knew.
    #[test]
    fn a_page_can_rest_on_a_steam_appid_and_its_executables_alone() {
        let mut stash = Stash::default();
        let key = stash.launch(
            "none",
            None,
            "wrapper:blops",
            Some("Call of Duty: Black Ops III"),
            "detectable",
            Confidence::High,
            Some("/games/bo3/BlackOps3.exe"),
            "2026-08-09",
        );
        stash.steam_sku(&key, 1985810);
        let pages = candidates(&stash.report, None);
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.canonical_id().as_deref(), Some("steam-1985810"));
        assert_eq!(page.exes, ["BlackOps3.exe"]);
        assert!(page.stores.is_empty());
        assert_eq!(page.status, Status::Ready);
        assert_eq!(page.file_name(), "call-of-duty-black-ops-iii.toml");
    }

    #[test]
    fn a_game_nothing_identifies_is_held_back_with_the_reason() {
        let mut stash = Stash::default();
        stash.launch(
            "none",
            None,
            "wrapper:mystery",
            Some("Mystery Game"),
            "mpris-hint",
            Confidence::Low,
            None,
            "2026-08-09",
        );
        let pages = candidates(&stash.report, None);
        assert_eq!(pages.len(), 1);
        assert_eq!(
            pages[0].status,
            Status::Incomplete {
                reason: "nothing identifies it: no store codename, no Steam appid, no executable"
                    .into()
            }
        );
        assert_eq!(pages[0].canonical_id(), None);
    }

    #[test]
    fn a_game_with_no_resolved_title_has_nothing_to_name_it() {
        let mut stash = Stash::default();
        stash.launch(
            "gog",
            Some("1660194629"),
            "gog:1660194629",
            None,
            "",
            Confidence::Low,
            Some("ProjectHospital.exe"),
            "2026-08-16",
        );
        let pages = candidates(&stash.report, None);
        assert!(
            matches!(&pages[0].status, Status::Incomplete { reason } if reason.contains("no title")),
            "{:?}",
            pages[0].status
        );
    }

    #[test]
    fn a_game_the_data_set_already_carries_is_never_written_twice() {
        let index = GamedbIndex::parse(super::super::index::FIXTURE).expect("fixture");

        // A page is caught through ANY identifier, not just the id: this one
        // has no Steam appid and still hits on its store codename.
        let mut stash = Stash::default();
        stash.launch(
            "egs",
            Some("Calluna"),
            "egs:Calluna",
            Some("Control"),
            "heroic-config",
            Confidence::High,
            None,
            "2026-08-15",
        );
        let pages = candidates(&stash.report, Some(&index));
        assert_eq!(
            pages[0].status,
            Status::InGamedb {
                id: "steam-870780".into(),
                title: "Control".into()
            }
        );

        // The executable route too - the Project Hospital page names one,
        // and the published entry already carries it.
        let mut stash = Stash::default();
        stash.launch(
            "gog",
            Some("1660194629"),
            "gog:1660194629",
            Some("Project Hospital"),
            "heroic-config",
            Confidence::High,
            Some("C:\\ProjectHospital\\ProjectHospital.exe"),
            "2026-08-16",
        );
        let pages = candidates(&stash.report, Some(&index));
        assert_eq!(
            pages[0].status,
            Status::InGamedb {
                id: "steam-868360".into(),
                title: "Project Hospital".into()
            }
        );
    }

    /// The real Control case. The published page carries the Epic codename
    /// and the Steam app id; this machine also saw a top-level executable.
    /// Holding the whole game back over that throws the executable away, so
    /// what comes back is an edit to the page rather than a hold-back.
    #[test]
    fn a_published_page_this_machine_knows_more_about_is_an_edit_not_a_hold_back() {
        let index = GamedbIndex::parse(super::super::index::FIXTURE).expect("fixture");
        let stash = control();
        let pages = candidates(&stash.report, Some(&index));
        assert_eq!(
            pages[0].status,
            Status::Enhance {
                id: "steam-870780".into(),
                title: "Control".into(),
                // Not the egs entry: the index already lists egs/Calluna.
                // Not the Steam app id either: games.steam is already 870780.
                additions: vec![Addition::Exe("Control_DX12.exe".into())],
            }
        );
    }

    /// A store the data set never saw, and the label a report prints for it.
    #[test]
    fn a_store_the_published_page_does_not_list_is_an_addition() {
        let index = GamedbIndex::parse(super::super::index::FIXTURE).expect("fixture");
        let mut stash = control();
        let key = stash.launch(
            "gog",
            Some("2049187585"),
            "gog:2049187585",
            Some("Control"),
            "heroic-library",
            Confidence::Medium,
            None,
            "2026-08-18",
        );
        stash.steam_sku(&key, 870780);
        let pages = candidates(&stash.report, Some(&index));
        let Status::Enhance { additions, .. } = &pages[0].status else {
            panic!("{:?}", pages[0].status);
        };
        assert_eq!(
            additions_label(additions),
            "+gog/2049187585, +exe Control_DX12.exe"
        );
    }

    /// An identifier that resolves to a DIFFERENT page is never an addition:
    /// writing it here would make two pages claim it.
    #[test]
    fn an_identifier_another_page_already_claims_is_never_added_to_this_one() {
        let index = GamedbIndex::parse(super::super::index::FIXTURE).expect("fixture");
        let mut stash = Stash::default();
        // Control's Epic copy, launched through a wrapper that reported
        // Project Hospital's executable - the mislabel helpers.toml exists
        // for, arriving from the other direction.
        stash.launch(
            "egs",
            Some("Calluna"),
            "egs:Calluna",
            Some("Control"),
            "heroic-config",
            Confidence::High,
            Some("C:\\ProjectHospital\\ProjectHospital.exe"),
            "2026-08-15",
        );
        let pages = candidates(&stash.report, Some(&index));
        assert_eq!(pages.len(), 1, "{pages:#?}");
        assert_eq!(
            pages[0].status,
            Status::InGamedb {
                id: "steam-870780".into(),
                title: "Control".into()
            },
            "an executable belonging to another page became an addition"
        );
    }

    /// The Steam app id is worth contributing exactly when the page has
    /// none. The fixture's Project Hospital page has one, so a candidate
    /// carrying the same fact adds nothing.
    #[test]
    fn a_steam_app_id_is_an_addition_only_where_the_page_has_none() {
        let raw = super::super::index::FIXTURE.replace(
            r#""page": "control", "year": null,
     "variant_of": null, "note": null, "steam": 870780"#,
            r#""page": "control", "year": null,
     "variant_of": null, "note": null, "steam": null"#,
        );
        let index = GamedbIndex::parse(&raw).expect("fixture");
        let stash = control();
        let pages = candidates(&stash.report, Some(&index));
        let Status::Enhance { additions, .. } = &pages[0].status else {
            panic!("{:?}", pages[0].status);
        };
        assert_eq!(
            additions_label(additions),
            "+exe Control_DX12.exe, +steam 870780"
        );
    }

    /// The gamedb pane's rows are games, but its correction keys act on
    /// stash entries, so the fold has to name which one.
    #[test]
    fn the_fold_names_the_entry_a_correction_should_land_on() {
        let stash = control();
        let pages = candidates(&stash.report, None);
        assert_eq!(pages[0].entry_keys.len(), 9);
        // Eight wrapper launches with no store, and one Epic entry with a
        // codename: the codename is what a title or store correction has to
        // land on, because it is the entry the fold reads first.
        assert_eq!(pages[0].rep_key, "egs:Calluna");

        // With nothing carrying a store identity, the newest entry wins.
        let mut stash = Stash::default();
        for (i, seen) in [(0, "2026-08-07"), (1, "2026-08-19")] {
            stash.launch(
                "none",
                None,
                &format!("wrapper:x-{i}"),
                Some("Mystery"),
                "detectable",
                Confidence::High,
                Some("Mystery.exe"),
                seen,
            );
        }
        let pages = candidates(&stash.report, None);
        assert_eq!(pages[0].rep_key, "wrapper:x-1");
    }

    #[test]
    fn a_correction_you_typed_is_recorded_as_a_manual_source() {
        let mut stash = Stash::default();
        let key = stash.launch(
            "none",
            Some("wrong"),
            "none:wrong",
            Some("Spellcraft"),
            "detectable",
            Confidence::High,
            Some("ProjectHospital.exe"),
            "2026-08-16",
        );
        stash.report.update(&key, |m| {
            m.store_override = Some("gog".into());
            m.codename_override = Some("1660194629".into());
            m.title_override = Some("Project Hospital".into());
        });
        let pages = candidates(&stash.report, None);
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        assert_eq!(page.title.as_deref(), Some("Project Hospital"));
        assert_eq!(page.canonical_id().as_deref(), Some("gog-1660194629"));
        assert_eq!(page.stores[0].source, "manual");
        // The executable belongs to THIS store's copy, so it rides along on
        // the store entry rather than becoming a page-level identifier.
        assert_eq!(page.stores[0].exe.as_deref(), Some("ProjectHospital.exe"));
        assert!(page.exes.is_empty());
    }

    #[test]
    fn the_store_precedence_decides_which_codename_becomes_the_id() {
        let mut stash = Stash::default();
        for (store, codename, seen) in [
            ("egs", "Calluna", "2026-08-15"),
            ("gog", "2049187585", "2026-08-14"),
        ] {
            stash.launch(
                store,
                Some(codename),
                &format!("{store}:{codename}"),
                Some("Control"),
                "heroic-library",
                Confidence::Medium,
                None,
                seen,
            );
        }
        // Two stores, two group keys, so two pages - the fold never guesses
        // that two codenames are one game. That judgement is a reviewer's.
        let pages = candidates(&stash.report, None);
        assert_eq!(pages.len(), 2);

        // With both entries on one page, gog wins by the documented order
        // even though the egs entry was seen more recently.
        let merged = Candidate {
            key: "x".into(),
            title: Some("Control".into()),
            steam: None,
            stores: vec![
                StoreEntry {
                    store: "egs".into(),
                    codename: "Calluna".into(),
                    exe: None,
                    seen: "2026-08-15".into(),
                    source: "heroic-library".into(),
                    confidence: "medium".into(),
                },
                StoreEntry {
                    store: "gog".into(),
                    codename: "2049187585".into(),
                    exe: None,
                    seen: "2026-08-14".into(),
                    source: "heroic-library".into(),
                    confidence: "medium".into(),
                },
            ],
            exes: Vec::new(),
            note: None,
            seen: "2026-08-15".into(),
            entries: 2,
            entry_keys: vec!["egs:Calluna".into(), "gog:2049187585".into()],
            rep_key: "egs:Calluna".into(),
            status: Status::Ready,
        };
        assert_eq!(merged.canonical_id().as_deref(), Some("gog-2049187585"));
    }

    #[test]
    fn a_dismissed_entry_is_out_of_the_pages_like_it_is_out_of_the_exports() {
        let mut stash = Stash::default();
        let key = stash.launch(
            "egs",
            Some("Calluna"),
            "egs:Calluna",
            Some("Control"),
            "heroic-config",
            Confidence::High,
            None,
            "2026-08-15",
        );
        stash
            .report
            .update(&key, |m| m.dismissed = Some("2026-08-20".into()));
        assert!(candidates(&stash.report, None).is_empty());
    }

    #[test]
    fn slugs_are_the_lowercase_title_and_nothing_else() {
        assert_eq!(slug("Control"), "control");
        assert_eq!(slug("Project Hospital"), "project-hospital");
        assert_eq!(
            slug("Call of Duty: Black Ops III"),
            "call-of-duty-black-ops-iii"
        );
        assert_eq!(slug("  S.T.A.L.K.E.R. 2  "), "s-t-a-l-k-e-r-2");
        assert_eq!(
            slug("Ori & the Will of the Wisps"),
            "ori-the-will-of-the-wisps"
        );
        assert_eq!(slug(""), "");
        assert_eq!(slug("---"), "");
    }

    /// The exact bytes, against the layout `gamedb/games/control.toml` uses.
    #[test]
    fn a_rendered_page_matches_the_data_sets_own_layout() {
        let stash = control();
        let pages = candidates(&stash.report, None);
        assert_eq!(
            render_page(&pages[0]),
            "title = \"Control\"\n\
             gamedb = \"steam-870780\"\n\
             note = \"\"\"\n\
             No protonfix upstream (checked 2026-08-22), so umu-database does not \\\n\
             want this game and it has no umu id.\"\"\"\n\
             exe = [\"Control_DX12.exe\"]\n\
             \n\
             [ids]\n\
             steam = 870780\n\
             source = \"steam-sku\"\n\
             seen = \"2026-08-15\"\n\
             \n\
             [[stores.egs]]\n\
             codename = \"Calluna\"\n\
             seen = \"2026-08-15\"\n\
             source = \"heroic-config\"\n\
             confidence = \"high\"\n"
        );
    }

    #[test]
    fn a_store_entrys_own_executable_is_written_beside_its_codename() {
        let candidate = Candidate {
            key: "gog-1660194629".into(),
            title: Some("Project Hospital".into()),
            steam: None,
            stores: vec![StoreEntry {
                store: "gog".into(),
                codename: "1660194629".into(),
                exe: Some("ProjectHospital.exe".into()),
                seen: "2026-08-16".into(),
                source: "manual".into(),
                confidence: "high".into(),
            }],
            exes: Vec::new(),
            note: None,
            seen: "2026-08-16".into(),
            entries: 1,
            entry_keys: vec!["gog:1660194629".into()],
            rep_key: "gog:1660194629".into(),
            status: Status::Ready,
        };
        assert_eq!(
            render_page(&candidate),
            "title = \"Project Hospital\"\n\
             gamedb = \"gog-1660194629\"\n\
             \n\
             [[stores.gog]]\n\
             codename = \"1660194629\"\n\
             exe = \"ProjectHospital.exe\"\n\
             seen = \"2026-08-16\"\n\
             source = \"manual\"\n\
             confidence = \"high\"\n"
        );
    }

    /// A launcher can put anything in a title, and a page that breaks its
    /// own quoting is worse than no page.
    #[test]
    fn a_title_with_quotes_and_backslashes_cannot_break_the_page() {
        let candidate = Candidate {
            key: "egs-X".into(),
            title: Some(r#"He said "hi"\z"#.into()),
            steam: None,
            stores: vec![StoreEntry {
                store: "egs".into(),
                codename: r#"a"b"#.into(),
                exe: None,
                seen: "2026-08-16".into(),
                source: "manual".into(),
                confidence: "low".into(),
            }],
            exes: Vec::new(),
            note: None,
            seen: "2026-08-16".into(),
            entries: 1,
            entry_keys: vec!["gog:1660194629".into()],
            rep_key: "gog:1660194629".into(),
            status: Status::Ready,
        };
        let text = render_page(&candidate);
        assert!(
            text.starts_with("title = \"He said \\\"hi\\\"\\\\z\"\n"),
            "{text}"
        );
        assert!(text.contains("codename = \"a\\\"b\"\n"), "{text}");
    }
}
