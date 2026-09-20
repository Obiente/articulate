use crate::{
    dictionary::{self, Entry},
    macros::{self, VoiceMacro},
};
use serde::{Deserialize, Serialize};

/// Portable user-authored vocabulary only. No transcripts, audio, or machine paths.
#[derive(Serialize, Deserialize)]
pub struct Library {
    version: u32,
    pub corrections: Vec<Entry>,
    pub macros: Vec<VoiceMacro>,
}

pub fn export(corrections: &[Entry], macros: &[VoiceMacro]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(&Library {
        version: 1,
        corrections: corrections.to_vec(),
        macros: macros.to_vec(),
    })?)
}

pub fn parse(input: &str) -> anyhow::Result<Library> {
    anyhow::ensure!(input.len() <= 1_000_000, "The library is too large");
    let library: Library = serde_json::from_str(input)
        .map_err(|_| anyhow::anyhow!("Paste a complete exported library"))?;
    anyhow::ensure!(
        library.version == 1,
        "This library needs a newer version of Transcribe"
    );
    anyhow::ensure!(
        library.corrections.len() <= 2000 && library.macros.len() <= 250,
        "Import up to 2,000 corrections and 250 macros at a time"
    );
    let mut corrections: Vec<Entry> = Vec::new();
    for entry in library.corrections {
        let mut valid = dictionary::validate(&entry.heard, &entry.wanted)?;
        valid.app = dictionary::app_scope(entry.app.as_deref().unwrap_or_default())?;
        valid.cues = dictionary::cue_words(&entry.cues.join(","))?;
        valid.ignore_case = entry.ignore_case;
        valid.enabled = entry.enabled;
        anyhow::ensure!(
            entry.contexts.len() <= 32,
            "Use up to 32 additional context rules per correction"
        );
        for context in entry.contexts {
            valid.contexts.push(dictionary::Context {
                cues: dictionary::cue_words(&context.cues.join(","))?,
                ignore_case: context.ignore_case,
            });
        }
        dictionary::save(&mut corrections, valid, None);
    }
    let mut macros: Vec<VoiceMacro> = Vec::new();
    for entry in library.macros {
        let mut valid = macros::validate(
            &entry.trigger,
            &entry.expansion,
            entry.app.as_deref().unwrap_or_default(),
        )?;
        valid.enabled = entry.enabled;
        anyhow::ensure!(
            !macros
                .iter()
                .any(|m| m.trigger == valid.trigger && m.app == valid.app),
            "The library has duplicate macros"
        );
        macros.push(valid);
    }
    Ok(Library {
        version: 1,
        corrections,
        macros,
    })
}

impl Library {
    pub fn merge(self, entries: &mut Vec<Entry>, macros: &mut Vec<VoiceMacro>) {
        for entry in self.corrections {
            dictionary::save(entries, entry, None);
        }
        for m in self.macros {
            macros.retain(|old| !(old.trigger == m.trigger && old.app == m.app));
            macros.push(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_library_keeps_scope_and_disabled_rules() {
        let mut entry = dictionary::validate("Jon", "John").unwrap();
        entry.app = Some("editor.exe".into());
        entry.cues = vec!["team".into()];
        entry.ignore_case = true;
        entry.enabled = false;
        let m = macros::validate("reply", "Hi,\n{text}", "").unwrap();
        let exported = export(&[entry.clone()], &[m]).unwrap();
        let library = parse(&exported).unwrap();
        assert!(library.corrections[0] == entry);
        assert_eq!(library.macros[0].expansion, "Hi,\n{text}");
        assert!(parse(&exported.replace("\"version\": 1", "\"version\": 2")).is_err());
        assert!(parse(&exported.replace("editor.exe", "C:/private/editor.exe")).is_err());
    }
    #[test]
    fn merge_preserves_different_spellings_and_other_apps() {
        let global = dictionary::validate("Jon", "John").unwrap();
        let mut scoped = global.clone();
        scoped.app = Some("editor.exe".into());
        let mut entries = vec![global, scoped.clone()];
        scoped.wanted = "Jonathan".into();
        let library = parse(&export(&[scoped], &[]).unwrap()).unwrap();
        library.merge(&mut entries, &mut vec![]);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].wanted, "John");
        assert_eq!(entries[1].wanted, "John");
        assert_eq!(entries[2].wanted, "Jonathan");
        assert_eq!(
            parse(&export(&[entries.clone(), entries].concat(), &[]).unwrap())
                .unwrap()
                .corrections
                .len(),
            3
        );
    }
    #[test]
    fn library_roundtrip_preserves_context_and_case_union() {
        let mut a = dictionary::validate("mercury", "Mercury").unwrap();
        a.cues = vec!["planet".into()];
        a.contexts.push(dictionary::Context {
            cues: vec!["function".into()],
            ignore_case: true,
        });
        let parsed = parse(&export(&[a.clone()], &[]).unwrap()).unwrap();
        assert!(parsed.corrections == vec![a]);
        assert_eq!(
            dictionary::apply("MERCURY planet", &parsed.corrections).1,
            0
        );
        assert_eq!(
            dictionary::apply("MERCURY function", &parsed.corrections).1,
            1
        );
    }
}
