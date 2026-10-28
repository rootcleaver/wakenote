//! Append only stable speech. Never backspace over text the user may have edited.
use std::collections::BTreeMap;

#[derive(Default)]
pub struct AutoTypeSession {
    chunks: BTreeMap<u64, Chunk>,
    corrected_finals: Vec<(u64, CorrectedFinal)>,
}

/// What happened when a confirmed chunk's final text corrected words already typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorrectedFinal {
    /// The remaining words were typed; the correction stays only in the saved transcript.
    RestTyped,
    /// The final text could not be lined up with the typed words; its untyped end was skipped.
    EndSkipped,
}

#[derive(Default)]
struct Chunk {
    previous: String,
    stable: String,
    typed: String,
    final_text: Option<String>,
    /// `stable` came from provider-confirmed words rather than partial agreement.
    confirmed: bool,
    /// Uncorrected confirmed text behind `stable` / `typed`.
    stable_raw: String,
    typed_raw: String,
    /// `stable_raw` corrected as a whole, which is how the final text starts.
    stable_raw_corrected: String,
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
    /// `correct` applies the same dictionary correction the saved transcript gets.
    pub fn confirm(&mut self, id: u64, raw: &str, correct: impl Fn(&str) -> String) {
        let Some(chunk) = self.chunks.get_mut(&id) else {
            return;
        };
        if chunk.final_text.is_some() {
            return;
        }
        let raw = raw.trim();
        if raw.len() <= chunk.stable_raw.len() || !raw.starts_with(chunk.typed_raw.as_str()) {
            return;
        }
        let corrected = correct(raw);
        chunk.stable_raw_corrected = corrected.clone();
        chunk.stable = if corrected.starts_with(&chunk.typed) {
            corrected
        } else {
            // A correction now spans words already typed (e.g. a two-word alias confirmed
            // one word at a time): keep what was typed and correct only the new words.
            format!("{}{}", chunk.typed, correct(&raw[chunk.typed_raw.len()..]))
        };
        chunk.stable_raw = raw.to_string();
        chunk.confirmed = true;
    }

    /// Confirmed chunks whose final text corrected words already typed.
    pub fn take_corrected_finals(&mut self) -> Vec<(u64, CorrectedFinal)> {
        std::mem::take(&mut self.corrected_finals)
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
                if chunk.confirmed {
                    // The final text corrects words already typed. Type the confirmed words
                    // still untyped, then the words the final adds after them (it starts with
                    // the confirmed text corrected as a whole). Keep later chunks.
                    let mut rest = chunk
                        .stable
                        .strip_prefix(&chunk.typed)
                        .unwrap_or_default()
                        .to_string();
                    let outcome = match target.strip_prefix(&chunk.stable_raw_corrected) {
                        Some(tail) => {
                            rest.push_str(tail);
                            CorrectedFinal::RestTyped
                        }
                        None => CorrectedFinal::EndSkipped,
                    };
                    if trailing_space && !(chunk.typed.is_empty() && rest.is_empty()) {
                        rest.push(' ');
                    }
                    self.chunks.remove(&id);
                    if !rest.is_empty() {
                        type_text(&rest)?;
                    }
                    self.corrected_finals.push((id, outcome));
                    continue;
                }
                self.chunks.remove(&id);
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
                let target = target.clone();
                let chunk = self.chunks.get_mut(&id).expect("active chunk");
                chunk.typed = target;
                chunk.typed_raw = chunk.stable_raw.clone();
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

    fn same(text: &str) -> String {
        text.to_string()
    }

    /// Dictionary with the two-word alias "wake note" -> "WakeNote".
    fn wakenote(text: &str) -> String {
        text.replace("wake note", "WakeNote")
    }

    #[test]
    fn alias_spanning_confirmed_words_keeps_every_word() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.confirm(1, "wake", wakenote);
        assert_eq!(typed(&mut session, false).unwrap(), "wake");
        // The alias now matches across the typed word; the new words still come through.
        session.confirm(1, "wake note is", wakenote);
        assert_eq!(typed(&mut session, false).unwrap(), " note is");
        session.confirm(1, "wake note is fast", wakenote);
        assert_eq!(typed(&mut session, false).unwrap(), " fast");
        // The saved final carries the correction; nothing is retyped or lost.
        session.finish(1, "WakeNote is fast");
        assert_eq!(typed(&mut session, true).unwrap(), " ");
        assert_eq!(
            session.take_corrected_finals(),
            vec![(1, CorrectedFinal::RestTyped)]
        );
    }

    #[test]
    fn final_only_words_after_a_spanning_alias_are_typed_once() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.confirm(1, "wake", wakenote);
        assert_eq!(typed(&mut session, false).unwrap(), "wake");
        session.confirm(1, "wake note is", wakenote);
        assert_eq!(typed(&mut session, false).unwrap(), " note is");
        // "fast" only appears in the final transcript.
        session.finish(1, "WakeNote is fast");
        assert_eq!(typed(&mut session, true).unwrap(), " fast ");
        assert_eq!(
            session.take_corrected_finals(),
            vec![(1, CorrectedFinal::RestTyped)]
        );
    }

    #[test]
    fn confirmed_words_waiting_behind_an_earlier_chunk_are_typed_corrected_once() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.start(2).unwrap();
        session.confirm(2, "wake", wakenote);
        session.confirm(2, "wake note done", wakenote);
        // Chunk 2 waits for chunk 1, so nothing of it is typed yet.
        assert_eq!(typed(&mut session, true).unwrap(), "");
        session.finish(1, "first");
        assert_eq!(typed(&mut session, true).unwrap(), "first WakeNote done");
        session.finish(2, "WakeNote done");
        assert_eq!(typed(&mut session, true).unwrap(), " ");
        assert!(session.take_corrected_finals().is_empty());
    }

    #[test]
    fn confirmed_words_type_as_they_grow_and_final_adds_only_the_rest() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.confirm(1, "내일 회의는 ", same);
        assert_eq!(typed(&mut session, false).unwrap(), "내일 회의는");
        session.confirm(1, "내일 회의는 3시에", same);
        assert_eq!(typed(&mut session, false).unwrap(), " 3시에");
        session.finish(1, "내일 회의는 3시에 해요.");
        assert_eq!(typed(&mut session, true).unwrap(), " 해요. ");
        assert!(session.take_corrected_finals().is_empty());
    }

    #[test]
    fn changed_final_skips_only_that_confirmed_chunk() {
        let mut session = AutoTypeSession::default();
        session.start(1).unwrap();
        session.start(2).unwrap();
        session.confirm(1, "wake note", same);
        assert_eq!(typed(&mut session, false).unwrap(), "wake note");
        session.finish(2, "둘째");
        // Dictionary post-processing rewrote confirmed words in the final text.
        session.finish(1, "WakeNote");
        assert_eq!(typed(&mut session, true).unwrap(), " 둘째 ");
        // "wake note" vs "WakeNote": nothing lines up, so no tail is guessed.
        assert_eq!(
            session.take_corrected_finals(),
            vec![(1, CorrectedFinal::EndSkipped)]
        );
    }

    #[test]
    fn confirm_ignores_unregistered_or_finished_chunks() {
        let mut session = AutoTypeSession::default();
        session.confirm(9, "never started", same);
        assert_eq!(typed(&mut session, false).unwrap(), "");
        session.start(1).unwrap();
        session.finish(1, "done");
        session.confirm(1, "done and more", same);
        assert_eq!(typed(&mut session, false).unwrap(), "done");
    }
}
