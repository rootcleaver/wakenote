//! Append only stable speech. Never backspace over text the user may have edited.
use std::collections::BTreeMap;

#[derive(Default)]
pub struct AutoTypeSession {
    chunks: BTreeMap<u64, Chunk>,
    dropped: Vec<u64>,
}

#[derive(Default)]
struct Chunk {
    previous: String,
    stable: String,
    typed: String,
    final_text: Option<String>,
    /// `stable` came from provider-confirmed words rather than partial agreement.
    confirmed: bool,
}

impl AutoTypeSession {
    pub fn start(&mut self, id: u64) -> Result<(), String> {
        if self.chunks.len() >= 32 {
            return Err("Auto-type is falling behind. Choose a faster model or check Activity for failed transcription.".into());
        }
        self.chunks.entry(id).or_default();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    /// Words the provider marked final never change, so they are typed as they grow.
    pub fn confirm(&mut self, id: u64, text: &str) {
        let Some(chunk) = self.chunks.get_mut(&id) else {
            return;
        };
        if chunk.final_text.is_some() {
            return;
        }
        let text = text.trim();
        if text.starts_with(&chunk.typed) && text.len() > chunk.stable.len() {
            chunk.stable = text.to_string();
            chunk.confirmed = true;
        }
    }

    /// Confirmed chunks whose final text no longer extends what was typed; their rest
    /// was skipped without stopping later chunks.
    pub fn take_dropped(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.dropped)
    }

    pub fn partial(&mut self, id: u64, text: &str) {
        let Some(chunk) = self.chunks.get_mut(&id) else {
            return;
        };
        if chunk.final_text.is_some() {
            return;
        }
        let text = text.trim_start();
        let prefix: String = chunk
            .previous
            .chars()
            .zip(text.chars())
            .take_while(|(left, right)| left == right)
            .map(|(ch, _)| ch)
            .collect();
        // Wait for a word boundary when available. CJK can make progress without spaces.
        let boundary = prefix
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map(|(index, ch)| index + ch.len_utf8())
            .unwrap_or(prefix.len());
        let stable = &prefix[..boundary];
        if stable.starts_with(&chunk.typed) && stable.len() > chunk.stable.len() {
            chunk.stable = stable.to_string();
        }
        chunk.previous = text.to_string();
    }

    pub fn finish(&mut self, id: u64, text: &str) {
        if let Some(chunk) = self.chunks.get_mut(&id) {
            chunk.final_text = Some(text.trim().to_string());
        }
    }

    pub fn discard(&mut self, id: u64) {
        self.chunks.remove(&id);
    }

    pub fn flush(
        &mut self,
        trailing_space: bool,
        mut type_text: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        while let Some((&id, chunk)) = self.chunks.first_key_value() {
            let is_final = chunk.final_text.is_some();
            let target = chunk.final_text.as_ref().unwrap_or(&chunk.stable);
            let Some(suffix) = target.strip_prefix(&chunk.typed) else {
                let confirmed = chunk.confirmed;
                self.chunks.remove(&id);
                if confirmed {
                    // Post-processing (e.g. dictionary) changed confirmed words; keep later chunks.
                    self.dropped.push(id);
                    continue;
                }
                return Err("Auto-type stopped this phrase because the model revised words already typed. The complete corrected transcript is available in Activity.".into());
            };
            let mut suffix = suffix.to_string();
            if is_final && trailing_space && !target.is_empty() {
                suffix.push(' ');
            }
            if !suffix.is_empty() {
                type_text(&suffix)?;
            }
            if is_final {
                self.chunks.remove(&id);
            } else {
                self.chunks.get_mut(&id).expect("active chunk").typed = target.clone();
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(session: &mut AutoTypeSession, trailing_space: bool) -> Result<String, String> {
        let mut out = String::new();
        session.flush(trailing_space, |text| {
            out.push_str(text);
            Ok(())
        })?;
        Ok(out)
    }

    #[test]
    fn stable_partial_then_revised_final_stops_phrase() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.partial(1, "내일 회의는 세 시");
        session.partial(1, "내일 회의는 세 시에 해");
        assert_eq!(typed(&mut session, false).unwrap(), "내일 회의는 세 ");
        session.finish(1, "내일 회의는 3시에 해요.");
        assert!(typed(&mut session, false).is_err());
    }

    #[test]
    fn final_only_types_the_whole_final_once() {
        // Final-only mode never feeds partials, so the final is typed as one piece.
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        assert_eq!(typed(&mut session, false).unwrap(), "");
        session.finish(1, "내일 회의는 3시에 해요.");
        assert_eq!(typed(&mut session, true).unwrap(), "내일 회의는 3시에 해요. ");
        assert_eq!(typed(&mut session, true).unwrap(), "");
    }

    #[test]
    fn final_only_keeps_chunk_order() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.start(2).unwrap();
        session.finish(2, "둘째");
        assert_eq!(typed(&mut session, false).unwrap(), "");
        session.finish(1, "첫째");
        assert_eq!(typed(&mut session, true).unwrap(), "첫째 둘째 ");
    }

    #[test]
    fn confirmed_words_type_as_they_grow_and_final_adds_only_the_rest() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.confirm(1, "내일 회의는 ");
        assert_eq!(typed(&mut session, false).unwrap(), "내일 회의는");
        session.confirm(1, "내일 회의는 3시에");
        assert_eq!(typed(&mut session, false).unwrap(), " 3시에");
        session.finish(1, "내일 회의는 3시에 해요.");
        assert_eq!(typed(&mut session, true).unwrap(), " 해요. ");
        assert!(session.take_dropped().is_empty());
    }

    #[test]
    fn changed_final_skips_only_that_confirmed_chunk() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.start(2).unwrap();
        session.confirm(1, "wake note");
        assert_eq!(typed(&mut session, false).unwrap(), "wake note");
        session.finish(2, "둘째");
        // Dictionary post-processing rewrote confirmed words in the final text.
        session.finish(1, "WakeNote");
        assert_eq!(typed(&mut session, true).unwrap(), "둘째 ");
        assert_eq!(session.take_dropped(), vec![1]);
    }

    #[test]
    fn confirm_ignores_unregistered_or_finished_chunks() {
        let mut session = AutoTypeSession::default();
        session.confirm(9, "never started");
        assert_eq!(typed(&mut session, false).unwrap(), "");
        session.start(1).unwrap();
        session.finish(1, "done");
        session.confirm(1, "done and more");
        assert_eq!(typed(&mut session, false).unwrap(), "done");
    }
}
