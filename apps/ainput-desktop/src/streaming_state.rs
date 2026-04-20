#[derive(Debug, Clone, Default)]
pub(crate) struct StreamingState {
    pub(crate) frozen_prefix: String,
    pub(crate) volatile_sentence: String,
    pub(crate) display_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamingDelta {
    pub(crate) display_text: String,
    pub(crate) rejected_prefix_rewrite: bool,
    pub(crate) stable_chars: usize,
    pub(crate) frozen_chars: usize,
    pub(crate) volatile_chars: usize,
}

impl StreamingState {
    pub(crate) fn full_text(&self) -> &str {
        &self.display_text
    }

    pub(crate) fn apply_online_partial(&mut self, candidate_text: &str) -> Option<StreamingDelta> {
        let candidate = candidate_text.trim();
        if candidate.is_empty() {
            return None;
        }

        let stable_chars = longest_common_prefix_chars(&self.display_text, candidate);
        let (candidate_frozen_prefix, _) = split_frozen_prefix(candidate);
        let next_frozen_prefix = if candidate_frozen_prefix.is_empty() {
            self.frozen_prefix.clone()
        } else if self.frozen_prefix.is_empty() {
            candidate_frozen_prefix
        } else if candidate_frozen_prefix.starts_with(&self.frozen_prefix) {
            candidate_frozen_prefix
        } else {
            return Some(self.snapshot_delta(stable_chars, true));
        };

        let next_volatile_sentence = if let Some(remainder) =
            match_candidate_after_frozen_prefix(candidate, &next_frozen_prefix)
        {
            remainder.to_string()
        } else if can_append_segment_only_candidate(candidate, &next_frozen_prefix) {
            candidate.to_string()
        } else {
            return Some(self.snapshot_delta(stable_chars, true));
        };

        self.set_segments(next_frozen_prefix, next_volatile_sentence);
        Some(self.snapshot_delta(stable_chars, false))
    }

    #[cfg(test)]
    pub(crate) fn apply_sentence_repair(&mut self, repaired_sentence: &str) -> StreamingDelta {
        let repaired = repaired_sentence.trim();
        let stable_chars = longest_common_prefix_chars(&self.display_text, repaired);
        let next_volatile_sentence = if repaired.is_empty() {
            String::new()
        } else {
            repaired.to_string()
        };
        self.set_segments(self.frozen_prefix.clone(), next_volatile_sentence);
        self.snapshot_delta(stable_chars, false)
    }

    pub(crate) fn finalize_from_streaming(&mut self, final_text: &str) -> StreamingDelta {
        let trimmed = final_text.trim();
        let stable_chars = longest_common_prefix_chars(&self.display_text, trimmed);
        let (frozen_prefix, volatile_sentence) = if trimmed.is_empty() {
            (self.frozen_prefix.clone(), self.volatile_sentence.clone())
        } else if match_candidate_after_frozen_prefix(trimmed, &self.frozen_prefix).is_some() {
            split_frozen_prefix(trimmed)
        } else if can_append_segment_only_candidate(trimmed, &self.frozen_prefix) {
            (self.frozen_prefix.clone(), trimmed.to_string())
        } else {
            split_frozen_prefix(trimmed)
        };
        self.set_segments(frozen_prefix, volatile_sentence);
        self.snapshot_delta(stable_chars, false)
    }

    pub(crate) fn freeze_with_committed_text(&mut self, committed_text: &str) -> StreamingDelta {
        let committed = committed_text.trim();
        let stable_chars = longest_common_prefix_chars(&self.display_text, committed);
        if committed.is_empty() {
            return self.snapshot_delta(stable_chars, false);
        }

        self.set_segments(committed.to_string(), String::new());
        self.snapshot_delta(stable_chars, false)
    }

    fn set_segments(&mut self, frozen_prefix: String, volatile_sentence: String) {
        self.frozen_prefix = frozen_prefix;
        self.volatile_sentence = volatile_sentence;
        self.display_text = format!("{}{}", self.frozen_prefix, self.volatile_sentence)
            .trim()
            .to_string();
    }

    fn snapshot_delta(&self, stable_chars: usize, rejected_prefix_rewrite: bool) -> StreamingDelta {
        StreamingDelta {
            display_text: self.display_text.clone(),
            rejected_prefix_rewrite,
            stable_chars,
            frozen_chars: self.frozen_prefix.chars().count(),
            volatile_chars: self.volatile_sentence.chars().count(),
        }
    }
}

pub(crate) fn longest_common_prefix_chars(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(lhs, rhs)| lhs == rhs)
        .count()
}

pub(crate) fn visible_text_char_count(text: &str) -> usize {
    text.trim()
        .trim_matches(|ch: char| ch.is_whitespace() || is_sentence_punctuation(ch))
        .chars()
        .count()
}

pub(crate) fn split_frozen_prefix(text: &str) -> (String, String) {
    let mut committed_end = 0usize;
    let mut trailing_after_boundary = false;

    for (index, ch) in text.char_indices() {
        let char_end = index + ch.len_utf8();
        if is_sentence_commit_char(ch) {
            committed_end = char_end;
            trailing_after_boundary = true;
            continue;
        }
        if trailing_after_boundary && is_sentence_trailing_char(ch) {
            committed_end = char_end;
            continue;
        }
        trailing_after_boundary = false;
    }

    let (committed, live) = text.split_at(committed_end);
    (committed.to_string(), live.to_string())
}

pub(crate) fn can_append_segment_only_candidate(candidate: &str, frozen_prefix: &str) -> bool {
    let candidate = candidate.trim();
    let relaxed_prefix = strip_final_sentence_boundary(frozen_prefix).trim();
    if candidate.is_empty() || relaxed_prefix.is_empty() {
        return false;
    }

    longest_common_prefix_chars(candidate, relaxed_prefix) < 2
}

fn match_candidate_after_frozen_prefix<'a>(
    candidate: &'a str,
    frozen_prefix: &str,
) -> Option<&'a str> {
    if frozen_prefix.is_empty() {
        return Some(candidate);
    }

    if let Some(remainder) = candidate.strip_prefix(frozen_prefix) {
        return Some(remainder);
    }

    let relaxed_prefix = strip_final_sentence_boundary(frozen_prefix);
    if relaxed_prefix.len() < frozen_prefix.len() {
        return candidate.strip_prefix(relaxed_prefix);
    }

    None
}

fn strip_final_sentence_boundary(text: &str) -> &str {
    let mut end = text.len();

    while let Some((index, ch)) = text[..end].char_indices().last() {
        if is_sentence_trailing_char(ch) {
            end = index;
            continue;
        }
        break;
    }

    while let Some((index, ch)) = text[..end].char_indices().last() {
        if is_sentence_commit_char(ch) {
            end = index;
            continue;
        }
        break;
    }

    &text[..end]
}

fn is_sentence_commit_char(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '!' | '?' | '；' | ';')
}

fn is_sentence_trailing_char(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '"'
            | '\''
            | '”'
            | '’'
            | ')'
            | '）'
            | ']'
            | '】'
            | '>'
            | '》'
            | '〉'
            | '」'
            | '』'
    )
}

fn is_sentence_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '.' | ','
            | '!'
            | '?'
            | ';'
            | ':'
            | '。'
            | '，'
            | '！'
            | '？'
            | '、'
            | '；'
            | '：'
            | '．'
            | '・'
    )
}

#[cfg(test)]
mod tests {
    use super::{StreamingState, can_append_segment_only_candidate, split_frozen_prefix};

    #[test]
    fn only_latest_sentence_can_be_rewritten() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句先是错字");
        let delta = state.apply_sentence_repair("第二句已经修正");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正");
        assert_eq!(state.frozen_prefix, "第一句已经稳定。");
        assert_eq!(state.volatile_sentence, "第二句已经修正");
    }

    #[test]
    fn completed_sentence_becomes_frozen_prefix() {
        let mut state = StreamingState::default();
        let delta = state
            .apply_online_partial("第一句已经稳定。第二句还在继续")
            .expect("online partial");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句还在继续");
        assert_eq!(state.frozen_prefix, "第一句已经稳定。");
        assert_eq!(state.volatile_sentence, "第二句还在继续");
    }

    #[test]
    fn comma_does_not_freeze_sentence_prefix() {
        let (frozen_prefix, volatile_sentence) = split_frozen_prefix("第一句还没说完，第二句继续");
        assert_eq!(frozen_prefix, "");
        assert_eq!(volatile_sentence, "第一句还没说完，第二句继续");
    }

    #[test]
    fn prefix_rewrite_is_rejected_after_sentence_is_frozen() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句继续");
        let delta = state
            .apply_online_partial("第一行已经稳定。第二句继续")
            .expect("state delta");
        assert!(delta.rejected_prefix_rewrite);
        assert_eq!(delta.display_text, "第一句已经稳定。第二句继续");
    }

    #[test]
    fn later_partial_can_continue_after_preview_added_terminal_punctuation() {
        let mut state = StreamingState::default();
        let first = state
            .apply_online_partial("第一句已经稳定吗？")
            .expect("first online partial");
        assert_eq!(first.display_text, "第一句已经稳定吗？");
        assert_eq!(state.frozen_prefix, "第一句已经稳定吗？");
        assert_eq!(state.volatile_sentence, "");

        let second = state
            .apply_online_partial("第一句已经稳定吗第二句继续")
            .expect("second online partial");
        assert!(!second.rejected_prefix_rewrite);
        assert_eq!(second.display_text, "第一句已经稳定吗？第二句继续");
        assert_eq!(state.frozen_prefix, "第一句已经稳定吗？");
        assert_eq!(state.volatile_sentence, "第二句继续");
    }

    #[test]
    fn finalize_from_streaming_keeps_latest_sentence_visible() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句先是错字");
        let delta = state.finalize_from_streaming("第一句已经稳定。第二句已经修正。");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正。");
        assert_eq!(state.frozen_prefix, "第一句已经稳定。第二句已经修正。");
        assert_eq!(state.volatile_sentence, "");
    }

    #[test]
    fn latest_sentence_only_partial_is_appended_after_frozen_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。");
        let delta = state
            .apply_online_partial("第二句继续")
            .expect("latest sentence only partial");
        assert!(!delta.rejected_prefix_rewrite);
        assert_eq!(delta.display_text, "第一句已经稳定。第二句继续");
        assert_eq!(state.frozen_prefix, "第一句已经稳定。");
        assert_eq!(state.volatile_sentence, "第二句继续");
    }

    #[test]
    fn latest_sentence_only_final_keeps_existing_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。");
        let delta = state.finalize_from_streaming("第二句已经修正。");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正。");
        assert_eq!(state.frozen_prefix, "第一句已经稳定。");
        assert_eq!(state.volatile_sentence, "第二句已经修正。");
    }

    #[test]
    fn freeze_with_committed_text_promotes_current_sentence_to_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句还没标点");
        let delta = state.freeze_with_committed_text("第一句还没标点。");
        assert_eq!(delta.display_text, "第一句还没标点。");
        assert_eq!(state.frozen_prefix, "第一句还没标点。");
        assert_eq!(state.volatile_sentence, "");
    }

    #[test]
    fn filler_after_punctuation_stays_in_latest_sentence() {
        let (frozen_prefix, volatile_sentence) =
            split_frozen_prefix("第一句已经稳定。嗯 第二句现在开始");
        assert_eq!(frozen_prefix, "第一句已经稳定。");
        assert_eq!(volatile_sentence, "嗯 第二句现在开始");
    }

    #[test]
    fn segment_only_detection_rejects_prefix_rewrites() {
        assert!(!can_append_segment_only_candidate(
            "第一行已经稳定。第二句继续",
            "第一句已经稳定。"
        ));
        assert!(can_append_segment_only_candidate(
            "第二句继续",
            "第一句已经稳定。"
        ));
    }
}
