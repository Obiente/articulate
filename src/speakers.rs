use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use transcribe_cpp::{
    Backend, Model, ModelOptions, RunExtension, RunOptions, Session, SessionOptions,
    SortformerPreset, SortformerStreamOptions,
};

pub const NAME: &str = "diar_streaming_sortformer_4spk-v2.1-F16.gguf";
pub fn path() -> std::path::PathBuf {
    crate::model::data_dir().join("models").join(NAME)
}

#[derive(Clone)]
struct Anchor {
    id: i32,
    pcm: Vec<f32>,
}
#[derive(Clone, Debug)]
pub struct Turn {
    pub start: usize,
    pub end: usize,
    pub speakers: Vec<i32>,
}
pub struct Tracker {
    session: Session,
    anchors: Vec<Anchor>,
    next: i32,
}

/// Acoustic references at the last committed transcript boundary. Native runs
/// reset their inference cache; only these references survive a run.
pub(crate) struct Checkpoint {
    anchors: Vec<Anchor>,
    next: i32,
}

impl Tracker {
    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            anchors: self.anchors.clone(),
            next: self.next,
        }
    }

    pub(crate) fn restore(&mut self, checkpoint: &Checkpoint) {
        self.anchors.clone_from(&checkpoint.anchors);
        self.next = checkpoint.next;
    }

    pub fn new(cpu: bool) -> Result<Self> {
        let model = Model::load_with(
            path(),
            &ModelOptions {
                backend: if cpu { Backend::Cpu } else { Backend::Auto },
                device: None,
            },
        )?;
        let session = model.session_with(&SessionOptions {
            n_threads: 6,
            ..Default::default()
        })?;
        Ok(Self {
            session,
            anchors: Vec::new(),
            next: 1,
        })
    }

    pub fn identify(&mut self, pcm: &[f32]) -> Result<Vec<Turn>> {
        if pcm.iter().all(|s| s.abs() < 0.00001) {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        let mut references = Vec::new();
        // The native diarizer's public API resets its cache on each run. Reuse
        // a bounded clean audio reference per speaker, not numeric local IDs.
        for anchor in &self.anchors {
            let start = input.len();
            input.extend_from_slice(&anchor.pcm);
            references.push((anchor.id, start / 16, input.len() / 16));
            input.resize(input.len() + 8000, 0.0);
        }
        let prefix = input.len();
        input.extend_from_slice(pcm);
        let result = self.session.run(
            &input,
            &RunOptions {
                family: Some(RunExtension::Sortformer(SortformerStreamOptions {
                    preset: Some(SortformerPreset::VeryHighLatency),
                })),
                ..Default::default()
            },
        )?;
        let mut mapping = BTreeMap::new();
        let mut votes = Vec::new();
        for &(global, start, end) in &references {
            let mut durations = BTreeMap::<i32, usize>::new();
            for segment in &result.speaker_segments {
                let overlap = (segment.t1_ms.max(0) as usize)
                    .min(end)
                    .saturating_sub((segment.t0_ms.max(0) as usize).max(start));
                *durations.entry(segment.speaker_id).or_default() += overlap;
            }
            for (local, duration) in durations {
                if duration > (end - start) / 2 {
                    votes.push((duration, local, global));
                }
            }
        }
        votes.sort_by_key(|v| std::cmp::Reverse(v.0));
        let mut used = BTreeSet::new();
        for (_, local, global) in votes {
            if !mapping.contains_key(&local) && used.insert(global) {
                mapping.insert(local, global);
            }
        }
        let mut spans = Vec::new();
        for segment in &result.speaker_segments {
            let end = (segment.t1_ms.max(0) as usize * 16)
                .saturating_sub(prefix)
                .min(pcm.len());
            let start = (segment.t0_ms.max(0) as usize * 16)
                .saturating_sub(prefix)
                .min(pcm.len());
            if end <= start {
                continue;
            }
            let id = *mapping.entry(segment.speaker_id).or_insert_with(|| {
                if used.len() < references.len() {
                    return 0;
                }
                if self.next <= 4 {
                    let id = self.next;
                    self.next += 1;
                    id
                } else {
                    0
                }
            });
            spans.push((start, end, id));
        }
        let turns = partition(&spans);
        for turn in &turns {
            if turn.speakers.len() != 1 || turn.speakers[0] <= 0 || turn.end - turn.start < 16000 {
                continue;
            }
            let id = turn.speakers[0];
            let clip = &pcm[turn.start..turn.end.min(turn.start + 64000)];
            if let Some(anchor) = self.anchors.iter_mut().find(|a| a.id == id) {
                if clip.len() > anchor.pcm.len() {
                    anchor.pcm = clip.to_vec();
                }
            } else {
                self.anchors.push(Anchor {
                    id,
                    pcm: clip.to_vec(),
                });
            }
        }
        Ok(turns)
    }
}

fn partition(spans: &[(usize, usize, i32)]) -> Vec<Turn> {
    let points: BTreeSet<_> = spans.iter().flat_map(|&(a, b, _)| [a, b]).collect();
    let points: Vec<_> = points.into_iter().collect();
    let mut turns: Vec<Turn> = Vec::new();
    for pair in points.windows(2) {
        let ids: BTreeSet<_> = spans
            .iter()
            .filter(|&&(a, b, _)| a < pair[1] && b > pair[0])
            .map(|s| s.2)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let ids: Vec<_> = ids.into_iter().collect();
        if let Some(last) = turns.last_mut()
            && last.end == pair[0]
            && last.speakers == ids
        {
            last.end = pair[1];
            continue;
        }
        turns.push(Turn {
            start: pair[0],
            end: pair[1],
            speakers: ids,
        });
    }
    turns
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlapping_speech_is_not_falsely_assigned_to_one_person() {
        let turns = partition(&[(0, 100, 1), (50, 150, 2)]);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[1].speakers, vec![1, 2]);
        assert_eq!((turns[1].start, turns[1].end), (50, 100));
    }
}
