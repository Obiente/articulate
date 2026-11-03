//! People named in conversations, kept on this device.
//!
//! Discord participants are remembered by their Discord user ID, with the
//! name Discord last reported, an avatar reference and any name you chose for
//! them, so later calls show the same name. Other names you give speakers are
//! kept as quick picks for renaming.
//!
//! Voices are never matched to people. The bundled diarizer (Sortformer v2.1
//! through transcribe.cpp 0.2.3) reports anonymous speakers numbered per
//! recording, with timings and a probability, but no speaker embedding that
//! could recognize the same voice in another recording.
//!
//! Stored in `people.json`, sealed for the current Windows account when
//! encryption at rest is on. Not included in backups. A damaged list is set
//! aside as `people.corrupt-<time>.json`, and an interrupted save can leave a
//! `.people.json.<n>.partial`; protection changes convert the set-aside
//! copies too, and forgetting everyone removes them.
use crate::{discord::avatar::Avatar, discord_attribution::NamedSpeaker, history::protect};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_PEOPLE: usize = 500;
const VERSION: u32 = 1;
/// Larger files are not a people list this version wrote.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The outcome of converting the people files to the current protection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Migration {
    pub changed: usize,
    pub failed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Copy {
    /// A damaged list moved aside.
    SetAside,
    /// What an interrupted atomic save left.
    Temporary,
}

/// Other files holding the list at `path`, or part of it.
fn copies(path: &Path) -> Vec<(PathBuf, Copy)> {
    let (Some(folder), Some(name), Some(stem)) = (
        path.parent(),
        path.file_name().and_then(|name| name.to_str()),
        path.file_stem().and_then(|stem| stem.to_str()),
    ) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let set_aside = format!("{stem}.corrupt-");
    let temporary = format!(".{name}.");
    entries
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().to_str()?.to_owned();
            let kind = if file.starts_with(&set_aside) && file.ends_with(".json") {
                Copy::SetAside
            } else if file.starts_with(&temporary) && file.ends_with(".partial") {
                Copy::Temporary
            } else {
                return None;
            };
            Some((entry.path(), kind))
        })
        .collect()
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscordLink {
    pub user_id: String,
    /// The name Discord reported most recently.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Avatar>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    pub name: String,
    /// The name was chosen here; Discord name changes do not replace it.
    #[serde(default)]
    pub custom_name: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord: Option<DiscordLink>,
    pub created_ms: u64,
    pub used_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    people: Vec<Person>,
}

/// The saved list. Mutations mark it dirty; `save` writes it.
pub struct Directory {
    /// `None` keeps the list in memory only.
    path: Option<PathBuf>,
    people: Vec<Person>,
    protect: bool,
    dirty: bool,
    /// Set when the file could not be read; it is never overwritten then.
    blocked: Option<String>,
}

fn new_id() -> String {
    use std::hash::{BuildHasher, RandomState};
    let seed = (crate::history::now_ms(), std::process::id());
    format!("{:016x}", RandomState::new().hash_one(seed))
}

impl Directory {
    pub fn path() -> PathBuf {
        crate::model::data_dir().join("people.json")
    }

    /// A list that is never written to disk.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            people: Vec::new(),
            protect: false,
            dirty: false,
            blocked: None,
        }
    }

    /// Load `path`, tolerating a missing file. A damaged file is moved aside.
    /// A list stored in the other protection is due to be rewritten; see
    /// `migrate`.
    pub fn load(path: PathBuf, protect: bool) -> Self {
        let mut directory = Self {
            path: Some(path.clone()),
            people: Vec::new(),
            protect,
            dirty: false,
            blocked: None,
        };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let sealed = protect::is_sealed(&bytes);
                let decoded = protect::open(bytes)
                    .and_then(|plain| Ok((sealed, serde_json::from_slice::<Stored>(&plain)?)));
                match decoded {
                    Ok((sealed, stored)) if stored.version <= VERSION => {
                        directory.people = stored.people;
                        directory.people.truncate(MAX_PEOPLE);
                        // The setting counts, not whatever form the file is in.
                        directory.dirty = sealed != protect;
                    }
                    Ok(_) => {
                        directory.blocked = Some(
                            "Known people were saved by a newer version of Articulate.".into(),
                        );
                    }
                    Err(error) if error.is::<protect::Undecryptable>() => {
                        directory.blocked = Some(error.to_string());
                    }
                    Err(_) => {
                        let aside = path
                            .with_extension(format!("corrupt-{}.json", crate::history::now_ms()));
                        let _ = std::fs::rename(&path, aside);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => directory.blocked = Some(format!("Could not read known people: {error}")),
        }
        directory
    }

    pub fn list(&self) -> &[Person] {
        &self.people
    }

    pub fn error(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn save(&mut self) -> Result<()> {
        if let Some(reason) = &self.blocked {
            anyhow::bail!("{reason}");
        }
        let Some(path) = &self.path else {
            self.dirty = false;
            return Ok(());
        };
        let plain = serde_json::to_vec_pretty(&Stored {
            version: VERSION,
            people: self.people.clone(),
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        protect::write_atomic(path, &protect::seal_if(self.protect, plain)?)
            .context("Could not save known people")?;
        self.dirty = false;
        Ok(())
    }

    /// Use `protect` for later saves. `migrate` converts what is stored.
    pub fn set_protect(&mut self, protect: bool) {
        if self.protect != protect {
            self.protect = protect;
            self.dirty |= !self.people.is_empty();
        }
    }

    /// Bring every stored copy to the current protection, judged from the
    /// files themselves: the list, copies set aside as damaged, and
    /// temporaries an interrupted save left (removed). Safe to repeat.
    pub fn migrate(&mut self) -> Migration {
        let mut migration = Migration::default();
        let Some(path) = self.path.clone() else {
            return migration;
        };
        let mut count = |result: Result<bool>| match result {
            Ok(true) => migration.changed += 1,
            Ok(false) => {}
            Err(_) => migration.failed += 1,
        };
        let stored = match std::fs::read(&path) {
            Ok(bytes) => Some(protect::is_sealed(&bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                count(Err(error.into()));
                Some(self.protect)
            }
        };
        let protect_list = self.protect;
        match stored {
            // A list this version cannot read is converted as it is.
            Some(sealed) if self.blocked.is_some() => {
                if sealed != protect_list {
                    count(protect::migrate_file(&path, protect_list, MAX_FILE_BYTES));
                }
            }
            Some(sealed) if sealed != protect_list || self.dirty => {
                count(self.save().map(|()| true));
            }
            None if self.dirty && self.blocked.is_none() => count(self.save().map(|()| true)),
            _ => {}
        }
        for (copy, kind) in copies(&path) {
            count(match kind {
                Copy::SetAside => protect::migrate_file(&copy, protect_list, MAX_FILE_BYTES),
                // Saves finish before returning, so none of these is in use.
                Copy::Temporary => remove_if_present(&copy).map(|()| false).map_err(Into::into),
            });
        }
        migration
    }

    fn touch(&mut self) {
        self.dirty = true;
    }

    /// Remember a Discord participant seen in a call. Returns whether anything changed.
    pub fn observe(&mut self, speaker: &NamedSpeaker) -> bool {
        let Some(user_id) = discord_id(speaker) else {
            return false;
        };
        let now = crate::history::now_ms();
        if let Some(person) = self.people.iter_mut().find(|person| {
            person
                .discord
                .as_ref()
                .is_some_and(|link| link.user_id == user_id)
        }) {
            let link = person.discord.as_mut().expect("matched above");
            let mut changed = false;
            if !speaker.name.trim().is_empty() && link.name != speaker.name {
                link.name.clone_from(&speaker.name);
                if !person.custom_name {
                    person.name.clone_from(&speaker.name);
                }
                changed = true;
            }
            if speaker.avatar.is_some() && link.avatar != speaker.avatar {
                link.avatar.clone_from(&speaker.avatar);
                changed = true;
            }
            // Recency only needs day precision; avoid rewriting every turn.
            if now.saturating_sub(person.used_ms) > 24 * 60 * 60 * 1000 {
                person.used_ms = now;
                changed = true;
            }
            if changed {
                self.touch();
            }
            return changed;
        }
        if speaker.name.trim().is_empty() || self.people.len() >= MAX_PEOPLE {
            return false;
        }
        self.people.push(Person {
            id: new_id(),
            name: speaker.name.clone(),
            custom_name: false,
            discord: Some(DiscordLink {
                user_id,
                name: speaker.name.clone(),
                avatar: speaker.avatar.clone(),
            }),
            created_ms: now,
            used_ms: now,
        });
        self.touch();
        true
    }

    /// The name chosen for a Discord participant, if one was.
    pub fn chosen_name(&self, speaker: &NamedSpeaker) -> Option<&str> {
        let user_id = discord_id(speaker)?;
        self.people
            .iter()
            .find(|person| {
                person.custom_name
                    && person
                        .discord
                        .as_ref()
                        .is_some_and(|link| link.user_id == user_id)
            })
            .map(|person| person.name.as_str())
    }

    /// Show chosen names on Discord-attributed rows. Returns whether any changed.
    pub fn apply(&self, rows: &mut [crate::calls::Row]) -> bool {
        let mut changed = false;
        for speaker in rows
            .iter_mut()
            .filter_map(|row| row.discord.as_mut())
            .flat_map(|attribution| attribution.speakers.iter_mut())
        {
            if let Some(name) = self.chosen_name(speaker)
                && speaker.name != name
            {
                speaker.name = name.to_owned();
                changed = true;
            }
        }
        changed
    }

    /// Choose a name for a Discord participant. Returns the previous person
    /// entry so the change can be undone with `restore`.
    pub fn name_participant(
        &mut self,
        speaker: &NamedSpeaker,
        name: &str,
    ) -> Option<Option<Person>> {
        let user_id = discord_id(speaker)?;
        let previous = self
            .people
            .iter()
            .find(|person| {
                person
                    .discord
                    .as_ref()
                    .is_some_and(|link| link.user_id == user_id)
            })
            .cloned();
        if previous.is_none() {
            self.observe(speaker);
        }
        let person = self.people.iter_mut().find(|person| {
            person
                .discord
                .as_ref()
                .is_some_and(|link| link.user_id == user_id)
        })?;
        person.name = name.to_owned();
        person.custom_name = true;
        person.used_ms = crate::history::now_ms();
        self.touch();
        Some(previous)
    }

    /// Undo `name_participant` for `user_id`.
    pub fn restore(&mut self, user_id: &str, previous: Option<Person>) {
        self.people.retain(|person| {
            person
                .discord
                .as_ref()
                .is_none_or(|link| link.user_id != user_id)
        });
        if let Some(person) = previous {
            self.people.push(person);
        }
        self.touch();
    }

    /// Keep a name as a quick pick. Existing names are only marked as used.
    pub fn remember_name(&mut self, name: &str) {
        let now = crate::history::now_ms();
        if let Some(person) = self
            .people
            .iter_mut()
            .find(|person| person.name.to_lowercase() == name.to_lowercase())
        {
            person.used_ms = now;
        } else if self.people.len() < MAX_PEOPLE {
            self.people.push(Person {
                id: new_id(),
                name: name.to_owned(),
                custom_name: true,
                discord: None,
                created_ms: now,
                used_ms: now,
            });
        }
        self.touch();
    }

    /// Add a person, or rename one by `id`.
    pub fn save_person(&mut self, id: Option<&str>, name: &str) -> Result<()> {
        let name = crate::speaker_edits::clean_name(name)?;
        match id {
            Some(id) => {
                let person = self
                    .people
                    .iter_mut()
                    .find(|person| person.id == id)
                    .context("This person is no longer in the list.")?;
                person.name = name;
                person.custom_name = true;
            }
            None => {
                ensure!(
                    !self
                        .people
                        .iter()
                        .any(|person| person.name.to_lowercase() == name.to_lowercase()),
                    "That name is already in the list."
                );
                ensure!(
                    self.people.len() < MAX_PEOPLE,
                    "The list of people is full."
                );
                self.remember_name(&name);
            }
        }
        self.touch();
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.people.len();
        self.people.retain(|person| person.id != id);
        let removed = self.people.len() != before;
        if removed {
            self.touch();
        }
        removed
    }

    pub fn clear(&mut self) {
        self.people.clear();
        self.touch();
    }

    /// Forget everyone: save an empty list, replacing even a file that could
    /// not be read, and remove damaged copies and leftovers of the list.
    pub fn forget_everyone(&mut self) -> Result<()> {
        self.clear();
        self.blocked = None;
        self.save()?;
        if let Some(path) = &self.path {
            for (copy, _) in copies(path) {
                remove_if_present(&copy)
                    .with_context(|| format!("Could not remove {}", copy.display()))?;
            }
        }
        Ok(())
    }
}

/// The Discord user ID of an attributed speaker: live rows carry it; saved
/// rows only keep it inside an avatar reference.
fn discord_id(speaker: &NamedSpeaker) -> Option<String> {
    if !speaker.id.is_empty() {
        return Some(speaker.id.clone());
    }
    speaker
        .avatar
        .as_ref()
        .filter(|avatar| avatar.valid())
        .map(|avatar| avatar.user_id.clone())
}

/// For tests and tools: a directory file under `folder`.
#[cfg(test)]
pub fn at(folder: &std::path::Path, protect: bool) -> Directory {
    Directory::load(folder.join("people.json"), protect)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "articulate-people-{label}-{}-{}",
            std::process::id(),
            crate::history::now_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn participant(id: &str, name: &str) -> NamedSpeaker {
        NamedSpeaker {
            id: id.into(),
            name: name.into(),
            avatar: None,
        }
    }

    fn row(speaker: NamedSpeaker) -> crate::calls::Row {
        crate::calls::Row {
            cues: Vec::new(),
            start_ms: 0,
            end_ms: 1000,
            microphone: false,
            speakers: Vec::new(),
            discord: Some(crate::discord_attribution::Attribution {
                generation: 1,
                channel_id: "synthetic".into(),
                speakers: vec![speaker],
            }),
            text: "Hello.".into(),
        }
    }

    #[test]
    fn discord_participants_keep_chosen_names_across_calls() {
        let dir = folder("discord");
        let mut people = at(&dir, false);
        assert!(people.observe(&participant("1001", "zoe_99")));
        assert!(!people.observe(&participant("1001", "zoe_99")));
        assert_eq!(people.list()[0].name, "zoe_99");
        let previous = people
            .name_participant(&participant("1001", "zoe_99"), "Zoë")
            .unwrap();
        people.save().unwrap();
        // A later call: a new directory instance reads the saved file.
        let later = at(&dir, false);
        let mut rows = vec![
            row(participant("1001", "zoe_99")),
            row(participant("2002", "sam")),
        ];
        assert!(later.apply(&mut rows));
        assert_eq!(crate::calls::label(&rows[0], &[]), "Zoë");
        assert_eq!(crate::calls::label(&rows[1], &[]), "sam");
        // A Discord rename does not replace the chosen name.
        people.observe(&participant("1001", "zoe_renamed"));
        assert_eq!(people.chosen_name(&participant("1001", "")), Some("Zoë"));
        people.restore("1001", previous);
        assert_eq!(people.chosen_name(&participant("1001", "")), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn quick_pick_names_can_be_managed_and_deleted() {
        let dir = folder("names");
        let mut people = at(&dir, true);
        people.remember_name("Morgan");
        people.remember_name("morgan");
        assert_eq!(people.list().len(), 1);
        people.save_person(None, "Casey").unwrap();
        assert!(people.save_person(None, "casey").is_err());
        let id = people.list()[1].id.clone();
        people.save_person(Some(&id), "Casey Jordan").unwrap();
        assert_eq!(people.list()[1].name, "Casey Jordan");
        people.save().unwrap();
        let sealed = std::fs::read(dir.join("people.json")).unwrap();
        assert!(protect::is_sealed(&sealed));
        assert!(!String::from_utf8_lossy(&sealed).contains("Morgan"));
        let mut reloaded = at(&dir, true);
        assert_eq!(reloaded.list().len(), 2);
        assert!(reloaded.remove(&id));
        assert_eq!(
            protect_as(&mut reloaded, false),
            Migration {
                changed: 1,
                failed: 0
            }
        );
        assert!(
            String::from_utf8_lossy(&std::fs::read(dir.join("people.json")).unwrap())
                .contains("Morgan")
        );
        reloaded.clear();
        reloaded.save().unwrap();
        assert!(at(&dir, false).list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_damaged_file_is_moved_aside_and_never_overwritten_silently() {
        let dir = folder("damaged");
        std::fs::write(dir.join("people.json"), b"{not json").unwrap();
        let people = at(&dir, false);
        assert!(people.list().is_empty() && people.error().is_none());
        assert!(!dir.join("people.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What an encryption change does: `set_protect`, then `migrate`.
    fn protect_as(people: &mut Directory, protect: bool) -> Migration {
        people.set_protect(protect);
        people.migrate()
    }

    fn sealed(path: &Path) -> bool {
        protect::is_sealed(&std::fs::read(path).unwrap())
    }

    #[test]
    fn startup_and_repeated_requests_fix_a_list_in_the_wrong_protection() {
        let dir = folder("mismatch");
        let file = dir.join("people.json");
        let mut people = at(&dir, false);
        people.remember_name("Morgan");
        people.save().unwrap();
        // Encryption is on, but the file was left in plaintext (say a
        // rewrite failed before the app closed).
        let mut people = at(&dir, true);
        assert!(people.dirty(), "the plaintext list is due to be rewritten");
        assert!(!sealed(&file));
        // Asking again for the current protection does not return early.
        assert_eq!(
            protect_as(&mut people, true),
            Migration {
                changed: 1,
                failed: 0
            }
        );
        assert!(sealed(&file) && !people.dirty());
        assert_eq!(people.migrate(), Migration::default());
        // A list this version cannot read is still converted as it is.
        let newer = br#"{"version":99,"people":[]}"#;
        std::fs::write(&file, newer).unwrap();
        let mut blocked = at(&dir, true);
        assert!(blocked.error().is_some());
        assert_eq!(blocked.migrate().changed, 1);
        assert!(sealed(&file));
        assert_eq!(protect::open(std::fs::read(&file).unwrap()).unwrap(), newer);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn set_aside_and_leftover_copies_follow_protection_and_forgetting() {
        let dir = folder("copies");
        let file = dir.join("people.json");
        std::fs::write(&file, b"{damaged Morgan").unwrap();
        let mut people = at(&dir, false);
        let aside = copies(&file);
        assert_eq!(aside.len(), 1);
        assert_eq!(aside[0].1, Copy::SetAside);
        let leftover = dir.join(".people.json.77-3.partial");
        std::fs::write(&leftover, br#"{"people":[{"name":"Casey"}]}"#).unwrap();
        std::fs::write(dir.join("settings.corrupt-1.json"), b"unrelated").unwrap();
        people.remember_name("Sam");
        let migration = protect_as(&mut people, true);
        assert_eq!(migration.failed, 0);
        assert!(sealed(&aside[0].0), "the damaged copy is sealed too");
        assert!(
            !leftover.exists(),
            "an interrupted save's leftover is removed"
        );
        assert!(sealed(&file));
        // Forgetting everyone leaves no copy of anyone behind.
        std::fs::write(&leftover, b"Casey").unwrap();
        people.forget_everyone().unwrap();
        assert!(people.list().is_empty());
        assert!(copies(&file).is_empty());
        assert!(dir.join("settings.corrupt-1.json").exists());
        assert!(at(&dir, true).list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
