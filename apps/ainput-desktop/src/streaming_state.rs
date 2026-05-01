const DEFAULT_MIN_AGREEMENT: usize = 2;
const DEFAULT_MAX_ROLLBACK_CHARS: usize = 8;
const MAX_HYPOTHESIS_HISTORY: usize = 6;

#[derive(Debug, Clone)]
pub(crate) struct StreamingState {
    pub(crate) committed_prefix: String,
    pub(crate) stable_tail: String,
    pub(crate) volatile_tail: String,
    pub(crate) rewrite_candidate: Option<String>,
    pub(crate) display_text: String,
    pub(crate) revision: u64,
    hypothesis_history: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingStabilityPolicy {
    pub(crate) min_agreement: usize,
    pub(crate) max_rollback_chars: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct StreamingDelta {
    pub(crate) display_text: String,
    pub(crate) rejected_prefix_rewrite: bool,
    pub(crate) stable_chars: usize,
    pub(crate) frozen_chars: usize,
    pub(crate) volatile_chars: usize,
    pub(crate) revision: u64,
}

impl Default for StreamingState {
    fn default() -> Self {
        Self {
            committed_prefix: String::new(),
            stable_tail: String::new(),
            volatile_tail: String::new(),
            rewrite_candidate: None,
            display_text: String::new(),
            revision: 0,
            hypothesis_history: Vec::new(),
        }
    }
}

impl Default for StreamingStabilityPolicy {
    fn default() -> Self {
        Self {
            min_agreement: DEFAULT_MIN_AGREEMENT,
            max_rollback_chars: DEFAULT_MAX_ROLLBACK_CHARS,
        }
    }
}

impl StreamingStabilityPolicy {
    fn normalized(self) -> Self {
        Self {
            min_agreement: self.min_agreement.clamp(1, MAX_HYPOTHESIS_HISTORY),
            max_rollback_chars: self.max_rollback_chars.min(64),
        }
    }
}

impl StreamingState {
    pub(crate) fn full_text(&self) -> &str {
        &self.display_text
    }

    pub(crate) fn current_tail(&self) -> String {
        self.rewrite_candidate
            .clone()
            .unwrap_or_else(|| format!("{}{}", self.stable_tail, self.volatile_tail))
    }

    #[cfg(test)]
    pub(crate) fn apply_online_partial(&mut self, candidate_text: &str) -> Option<StreamingDelta> {
        self.apply_online_partial_with_policy(candidate_text, StreamingStabilityPolicy::default())
    }

    pub(crate) fn apply_online_partial_with_policy(
        &mut self,
        candidate_text: &str,
        policy: StreamingStabilityPolicy,
    ) -> Option<StreamingDelta> {
        let candidate = candidate_text.trim();
        if candidate.is_empty() {
            return None;
        }

        let previous_display = self.display_text.clone();
        let (next_committed_prefix, next_tail) =
            match self.resolve_candidate_segments(candidate, true) {
                Some(segments) => segments,
                None => return Some(self.snapshot_delta(&previous_display, true)),
            };

        let rejected = self.apply_tail_hypothesis(next_committed_prefix, next_tail, policy);
        Some(self.snapshot_delta(&previous_display, rejected))
    }

    #[cfg(test)]
    pub(crate) fn apply_sentence_repair(&mut self, repaired_sentence: &str) -> StreamingDelta {
        let previous_display = self.display_text.clone();
        let repaired = repaired_sentence.trim();
        self.set_segments(
            self.committed_prefix.clone(),
            String::new(),
            repaired.to_string(),
            None,
        );
        self.snapshot_delta(&previous_display, false)
    }

    #[cfg(test)]
    pub(crate) fn apply_rewrite_candidate(
        &mut self,
        committed_prefix: &str,
        rewritten_tail: &str,
        expected_revision: u64,
    ) -> Option<StreamingDelta> {
        if self.revision != expected_revision {
            return None;
        }

        let committed_prefix = committed_prefix.trim();
        if committed_prefix != self.committed_prefix.trim() {
            return None;
        }

        let rewritten_tail = rewritten_tail.trim();
        if rewritten_tail.is_empty() {
            return None;
        }

        let previous_display = self.display_text.clone();
        self.set_segments(
            self.committed_prefix.clone(),
            self.stable_tail.clone(),
            self.volatile_tail.clone(),
            Some(rewritten_tail.to_string()),
        );
        Some(self.snapshot_delta(&previous_display, false))
    }

    pub(crate) fn finalize_from_streaming(&mut self, final_text: &str) -> StreamingDelta {
        let previous_display = self.display_text.clone();
        let trimmed = final_text.trim();
        if trimmed.is_empty() {
            return self.snapshot_delta(&previous_display, false);
        }

        let next_committed_prefix = self.committed_prefix.clone();
        let next_tail = if let Some(remainder) =
            match_candidate_after_frozen_prefix(trimmed, &next_committed_prefix)
        {
            remainder.to_string()
        } else if can_append_segment_only_candidate(trimmed, &next_committed_prefix) {
            trimmed.to_string()
        } else {
            return self.snapshot_delta(&previous_display, true);
        };

        self.set_segments(next_committed_prefix, next_tail, String::new(), None);
        self.snapshot_delta(&previous_display, false)
    }

    pub(crate) fn freeze_with_committed_text(&mut self, committed_text: &str) -> StreamingDelta {
        let previous_display = self.display_text.clone();
        let committed = committed_text.trim();
        if committed.is_empty() {
            return self.snapshot_delta(&previous_display, false);
        }

        self.hypothesis_history.clear();
        self.set_segments(committed.to_string(), String::new(), String::new(), None);
        self.snapshot_delta(&previous_display, false)
    }

    pub(crate) fn rollover_with_display_text(&mut self, display_text: &str) -> StreamingDelta {
        let previous_display = self.display_text.clone();
        let display = display_text.trim();
        if display.is_empty() {
            return self.snapshot_delta(&previous_display, false);
        }

        let (candidate_committed_prefix, _) = split_frozen_prefix(display);
        let next_committed_prefix = if candidate_committed_prefix.is_empty() {
            self.committed_prefix.clone()
        } else if self.committed_prefix.is_empty()
            || candidate_committed_prefix.starts_with(&self.committed_prefix)
        {
            candidate_committed_prefix
        } else {
            self.committed_prefix.clone()
        };

        let next_tail = if let Some(remainder) =
            match_candidate_after_frozen_prefix(display, &next_committed_prefix)
        {
            remainder.to_string()
        } else if can_append_segment_only_candidate(display, &next_committed_prefix) {
            display.to_string()
        } else {
            self.current_tail()
        };

        self.hypothesis_history.clear();
        self.set_segments(next_committed_prefix, next_tail, String::new(), None);
        self.snapshot_delta(&previous_display, false)
    }

    fn resolve_candidate_segments(
        &self,
        candidate: &str,
        allow_segment_only: bool,
    ) -> Option<(String, String)> {
        let (candidate_committed_prefix, _) = split_frozen_prefix(candidate);
        let next_committed_prefix = if candidate_committed_prefix.is_empty() {
            self.committed_prefix.clone()
        } else if self.committed_prefix.is_empty() {
            candidate_committed_prefix
        } else if candidate_committed_prefix.starts_with(&self.committed_prefix) {
            candidate_committed_prefix
        } else {
            return None;
        };

        let next_tail = if let Some(remainder) =
            match_candidate_after_frozen_prefix(candidate, &next_committed_prefix)
        {
            remainder.to_string()
        } else if allow_segment_only
            && can_append_segment_only_candidate(candidate, &next_committed_prefix)
        {
            candidate.to_string()
        } else {
            return None;
        };

        Some((next_committed_prefix, next_tail))
    }

    fn apply_tail_hypothesis(
        &mut self,
        next_committed_prefix: String,
        candidate_tail: String,
        policy: StreamingStabilityPolicy,
    ) -> bool {
        let policy = policy.normalized();
        let committed_changed = next_committed_prefix != self.committed_prefix;
        let current_stable_tail = if committed_changed {
            String::new()
        } else {
            self.stable_tail.clone()
        };

        let rollback_base_chars =
            longest_common_prefix_chars(&current_stable_tail, &candidate_tail);
        let stable_chars = current_stable_tail.chars().count();
        let rollback_chars = stable_chars.saturating_sub(rollback_base_chars);
        if rollback_chars > policy.max_rollback_chars {
            return true;
        }

        if committed_changed {
            self.hypothesis_history.clear();
        }
        self.hypothesis_history.push(candidate_tail.clone());
        if self.hypothesis_history.len() > MAX_HYPOTHESIS_HISTORY {
            let overflow = self.hypothesis_history.len() - MAX_HYPOTHESIS_HISTORY;
            self.hypothesis_history.drain(..overflow);
        }

        let rollback_base = take_prefix_chars(&current_stable_tail, rollback_base_chars);
        let agreed_tail = agreed_hypothesis_prefix(&self.hypothesis_history, policy.min_agreement);
        let next_stable_tail = if agreed_tail.starts_with(&rollback_base) {
            agreed_tail
        } else {
            rollback_base
        };
        let next_volatile_tail = candidate_tail
            .strip_prefix(&next_stable_tail)
            .unwrap_or("")
            .to_string();

        self.set_segments(
            next_committed_prefix,
            next_stable_tail,
            next_volatile_tail,
            None,
        );
        false
    }

    fn set_segments(
        &mut self,
        committed_prefix: String,
        stable_tail: String,
        volatile_tail: String,
        rewrite_candidate: Option<String>,
    ) {
        let next_display_text = format!(
            "{}{}",
            committed_prefix,
            rewrite_candidate
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}{}", stable_tail, volatile_tail))
        )
        .trim()
        .to_string();
        let changed = self.committed_prefix != committed_prefix
            || self.stable_tail != stable_tail
            || self.volatile_tail != volatile_tail
            || self.rewrite_candidate != rewrite_candidate
            || self.display_text != next_display_text;

        self.committed_prefix = committed_prefix;
        self.stable_tail = stable_tail;
        self.volatile_tail = volatile_tail;
        self.rewrite_candidate = rewrite_candidate;
        self.display_text = next_display_text;
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
    }

    fn snapshot_delta(
        &self,
        previous_display: &str,
        rejected_prefix_rewrite: bool,
    ) -> StreamingDelta {
        StreamingDelta {
            display_text: self.display_text.clone(),
            rejected_prefix_rewrite,
            stable_chars: longest_common_prefix_chars(previous_display, &self.display_text),
            frozen_chars: self.committed_prefix.chars().count(),
            volatile_chars: self.current_tail().chars().count(),
            revision: self.revision,
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

fn agreed_hypothesis_prefix(history: &[String], min_agreement: usize) -> String {
    if history.len() < min_agreement.max(1) {
        return String::new();
    }

    let recent = &history[history.len() - min_agreement.max(1)..];
    let mut prefix = recent[0].clone();
    for hypothesis in &recent[1..] {
        let common_chars = longest_common_prefix_chars(&prefix, hypothesis);
        prefix = take_prefix_chars(&prefix, common_chars);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
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

fn take_prefix_chars(text: &str, char_count: usize) -> String {
    text.chars().take(char_count).collect()
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
    use super::{
        StreamingStabilityPolicy, StreamingState, can_append_segment_only_candidate,
        split_frozen_prefix,
    };

    #[test]
    fn only_latest_sentence_can_be_rewritten() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句先是错字");
        let delta = state.apply_sentence_repair("第二句已经修正");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.current_tail(), "第二句已经修正");
    }

    #[test]
    fn completed_sentence_becomes_committed_prefix() {
        let mut state = StreamingState::default();
        let delta = state
            .apply_online_partial("第一句已经稳定。第二句还在继续")
            .expect("online partial");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句还在继续");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.volatile_tail, "第二句还在继续");
    }

    #[test]
    fn local_agreement_promotes_stable_tail_after_two_matching_hypotheses() {
        let mut state = StreamingState::default();
        state.apply_online_partial("帮我看一下这个功能");
        assert_eq!(state.stable_tail, "");
        assert_eq!(state.volatile_tail, "帮我看一下这个功能");

        state.apply_online_partial("帮我看一下这个功能有没有问题");
        assert_eq!(state.stable_tail, "帮我看一下这个功能");
        assert_eq!(state.volatile_tail, "有没有问题");
    }

    #[test]
    fn large_stable_tail_rewrite_is_rejected() {
        let mut state = StreamingState::default();
        state.apply_online_partial("帮我看一下这个功能有没有问题");
        state.apply_online_partial("帮我看一下这个功能有没有问题");
        let before = state.display_text.clone();
        let delta = state
            .apply_online_partial_with_policy(
                "完全改掉前面的稳定内容",
                StreamingStabilityPolicy {
                    min_agreement: 1,
                    max_rollback_chars: 2,
                },
            )
            .expect("delta");
        assert!(delta.rejected_prefix_rewrite);
        assert_eq!(delta.display_text, before);
    }

    #[test]
    fn comma_does_not_freeze_sentence_prefix() {
        let (committed_prefix, volatile_sentence) =
            split_frozen_prefix("第一句还没说完，第二句继续");
        assert_eq!(committed_prefix, "");
        assert_eq!(volatile_sentence, "第一句还没说完，第二句继续");
    }

    #[test]
    fn prefix_rewrite_is_rejected_after_sentence_is_committed() {
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
        assert_eq!(state.committed_prefix, "第一句已经稳定吗？");
        assert_eq!(state.current_tail(), "");

        let second = state
            .apply_online_partial("第一句已经稳定吗第二句继续")
            .expect("second online partial");
        assert!(!second.rejected_prefix_rewrite);
        assert_eq!(second.display_text, "第一句已经稳定吗？第二句继续");
        assert_eq!(state.committed_prefix, "第一句已经稳定吗？");
        assert_eq!(state.current_tail(), "第二句继续");
    }

    #[test]
    fn finalize_from_streaming_updates_latest_sentence_without_committing_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句先是错字");
        let delta = state.finalize_from_streaming("第一句已经稳定。第二句已经修正。");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正。");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.stable_tail, "第二句已经修正。");
    }

    #[test]
    fn latest_sentence_only_partial_is_appended_after_committed_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。");
        let delta = state
            .apply_online_partial("第二句继续")
            .expect("latest sentence only partial");
        assert!(!delta.rejected_prefix_rewrite);
        assert_eq!(delta.display_text, "第一句已经稳定。第二句继续");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.current_tail(), "第二句继续");
    }

    #[test]
    fn latest_sentence_only_final_keeps_existing_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。");
        let delta = state.finalize_from_streaming("第二句已经修正。");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正。");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.stable_tail, "第二句已经修正。");
    }

    #[test]
    fn final_candidate_cannot_rewrite_existing_committed_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句继续");
        let delta = state.finalize_from_streaming("第一行已经稳定。第二句最终修正。");
        assert!(delta.rejected_prefix_rewrite);
        assert_eq!(delta.display_text, "第一句已经稳定。第二句继续");
        assert_eq!(state.committed_prefix, "第一句已经稳定。");
        assert_eq!(state.current_tail(), "第二句继续");
    }

    #[test]
    fn freeze_with_committed_text_promotes_current_sentence_to_prefix() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句还没标点");
        let delta = state.freeze_with_committed_text("第一句还没标点。");
        assert_eq!(delta.display_text, "第一句还没标点。");
        assert_eq!(state.committed_prefix, "第一句还没标点。");
        assert_eq!(state.current_tail(), "");
    }

    #[test]
    fn rollover_without_sentence_boundary_keeps_tail_live() {
        let mut state = StreamingState::default();
        state.apply_online_partial("这个功能应该是这样");
        let delta = state.rollover_with_display_text("这个功能应该是这样");
        assert_eq!(delta.display_text, "这个功能应该是这样");
        assert_eq!(state.committed_prefix, "");
        assert_eq!(state.current_tail(), "这个功能应该是这样");

        let update = state
            .apply_online_partial("这个功能应该是这样了")
            .expect("tail can still update after rollover");
        assert!(!update.rejected_prefix_rewrite);
        assert_eq!(update.display_text, "这个功能应该是这样了");
    }

    #[test]
    fn rollover_commits_only_existing_sentence_boundary() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经结束。第二句还没结束");
        let delta = state.rollover_with_display_text("第一句已经结束。第二句还没结束");
        assert_eq!(delta.display_text, "第一句已经结束。第二句还没结束");
        assert_eq!(state.committed_prefix, "第一句已经结束。");
        assert_eq!(state.current_tail(), "第二句还没结束");
    }

    #[test]
    fn rewrite_candidate_is_revision_guarded() {
        let mut state = StreamingState::default();
        state.apply_online_partial("第一句已经稳定。第二句先是错字");
        let revision = state.revision;
        let delta = state
            .apply_rewrite_candidate("第一句已经稳定。", "第二句已经修正", revision)
            .expect("rewrite delta");
        assert_eq!(delta.display_text, "第一句已经稳定。第二句已经修正");
        assert!(
            state
                .apply_rewrite_candidate("第一句已经稳定。", "第二句再次修正", revision)
                .is_none()
        );
    }

    #[test]
    fn filler_after_punctuation_stays_in_latest_sentence() {
        let (committed_prefix, volatile_sentence) =
            split_frozen_prefix("第一句已经稳定。嗯 第二句现在开始");
        assert_eq!(committed_prefix, "第一句已经稳定。");
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
