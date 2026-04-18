const DUPLICATE_PHRASES: &[&str] = &[
    "我", "我们", "你", "你们", "他", "她", "这个", "那个", "就是", "然后", "所以", "可以",
];
const STREAMING_FILLER_PREFIXES: &[&str] = &["嗯", "呃", "额", "啊"];
const STREAMING_SEGMENT_MIN_CHARS: usize = 6;
const STREAMING_SEGMENT_HARD_LIMIT: usize = 28;

pub fn normalize_transcription(text: &str) -> String {
    let mut current = collapse_whitespace(text);
    current = collapse_known_duplicates(&current);
    current = cleanup_punctuation_spacing(&current);

    current.trim().to_string()
}

pub fn normalize_streaming_preview(text: &str) -> String {
    let mut current = normalize_transcription(text);
    current = trim_streaming_fillers(&current);
    current.trim().to_string()
}

pub fn rewrite_streaming_text(text: &str) -> Vec<String> {
    let normalized = normalize_streaming_preview(text);
    if normalized.is_empty() {
        return Vec::new();
    }

    let raw_segments = split_streaming_segments(&normalized);
    let total = raw_segments.len();
    raw_segments
        .into_iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let rewritten = finalize_streaming_segment(&segment, index + 1 == total);
            (!rewritten.is_empty()).then_some(rewritten)
        })
        .collect()
}

fn collapse_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_was_space = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !previous_was_space {
                result.push(' ');
                previous_was_space = true;
            }
        } else {
            result.push(ch);
            previous_was_space = false;
        }
    }

    result.trim().to_string()
}

fn collapse_known_duplicates(text: &str) -> String {
    let mut current = text.to_string();

    for phrase in DUPLICATE_PHRASES {
        loop {
            let collapsed = current.replace(&format!("{phrase}{phrase}"), phrase);
            if collapsed == current {
                break;
            }
            current = collapsed;
        }
    }

    current
}

fn cleanup_punctuation_spacing(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        if *ch == ' ' {
            let prev = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
            let next = chars.get(index + 1).copied();

            if prev.is_some_and(is_cjk_punctuation) || next.is_some_and(is_cjk_punctuation) {
                continue;
            }
        }

        result.push(*ch);
    }

    result
}

fn trim_streaming_fillers(text: &str) -> String {
    let mut current = text.trim().to_string();

    loop {
        let mut trimmed = false;
        for filler in STREAMING_FILLER_PREFIXES {
            if let Some(rest) = strip_filler_prefix(&current, filler) {
                current = rest.to_string();
                trimmed = true;
                break;
            }
        }

        if !trimmed {
            break;
        }
    }

    current.trim().to_string()
}

fn strip_filler_prefix<'a>(text: &'a str, filler: &str) -> Option<&'a str> {
    let remainder = text.strip_prefix(filler)?;
    if remainder.is_empty() {
        return Some(remainder);
    }

    let next = remainder.chars().next()?;
    if matches!(next, ' ' | ',' | '，' | '.' | '。' | '!' | '！' | '?' | '？') {
        return Some(remainder.trim_start_matches(is_prefix_separator));
    }

    None
}

fn split_streaming_segments(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut segments = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let hard_end = (start + STREAMING_SEGMENT_HARD_LIMIT).min(chars.len());
        let end = find_segment_boundary(&chars, start, hard_end).unwrap_or(hard_end);
        let segment: String = chars[start..end].iter().collect();
        let segment = segment.trim_matches(is_soft_separator).trim().to_string();
        if !segment.is_empty() {
            segments.push(segment);
        }
        start = end;
        while start < chars.len() && is_soft_separator(chars[start]) {
            start += 1;
        }
    }

    segments
}

fn find_segment_boundary(
    chars: &[char],
    start: usize,
    hard_end: usize,
) -> Option<usize> {
    if hard_end.saturating_sub(start) <= STREAMING_SEGMENT_MIN_CHARS {
        return Some(hard_end);
    }

    for index in (start + STREAMING_SEGMENT_MIN_CHARS..hard_end).rev() {
        if is_strong_boundary(chars[index - 1]) {
            return Some(index);
        }
    }

    for index in (start + STREAMING_SEGMENT_MIN_CHARS..hard_end).rev() {
        if is_soft_boundary(chars[index - 1]) {
            return Some(index);
        }
    }

    None
}

fn finalize_streaming_segment(text: &str, is_final: bool) -> String {
    let mut current = normalize_streaming_preview(text);
    if current.is_empty() {
        return current;
    }

    if has_terminal_punctuation(&current) {
        return current;
    }

    let punctuation = infer_streaming_terminal_punctuation(&current, is_final);
    current.push(punctuation);
    current
}

fn infer_streaming_terminal_punctuation(text: &str, is_final: bool) -> char {
    let trimmed = text.trim();
    if trimmed.ends_with('吗')
        || trimmed.ends_with('么')
        || trimmed.contains("是不是")
        || trimmed.contains("对不对")
        || trimmed.contains("要不要")
    {
        '？'
    } else if trimmed.ends_with('吧')
        || trimmed.ends_with('呀')
        || trimmed.ends_with('啊')
        || trimmed.ends_with('啦')
    {
        if is_final { '。' } else { '，' }
    } else if is_final {
        '。'
    } else {
        '，'
    }
}

fn has_terminal_punctuation(text: &str) -> bool {
    text.chars()
        .last()
        .is_some_and(|ch| matches!(ch, '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';'))
}

fn is_strong_boundary(ch: char) -> bool {
    matches!(ch, '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';')
}

fn is_soft_boundary(ch: char) -> bool {
    is_strong_boundary(ch) || is_soft_separator(ch)
}

fn is_soft_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '、' | '/' | '|' | '-' | '—')
}

fn is_prefix_separator(ch: char) -> bool {
    is_soft_separator(ch) || matches!(ch, ',' | '，' | '.' | '。' | '!' | '！' | '?' | '？')
}

fn is_cjk_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。' | '！' | '？' | '：' | '；' | '、' | '）' | '】' | '》'
    )
}

#[cfg(test)]
mod tests {
    use super::{normalize_streaming_preview, normalize_transcription, rewrite_streaming_text};

    #[test]
    fn keeps_leading_fillers_and_original_opening_words() {
        assert_eq!(normalize_transcription("嗯，帮我看一下"), "嗯，帮我看一下");
        assert_eq!(normalize_transcription("呃，帮我看一下"), "呃，帮我看一下");
        assert_eq!(normalize_transcription("额，帮我看一下"), "额，帮我看一下");
        assert_eq!(normalize_transcription("那个那个这个问题"), "那个这个问题");
        assert_eq!(normalize_transcription("就是就是这个问题"), "就是这个问题");
    }

    #[test]
    fn keeps_single_leading_words_that_can_be_semantic() {
        assert_eq!(normalize_transcription("那个问题要改"), "那个问题要改");
        assert_eq!(normalize_transcription("就是这个问题"), "就是这个问题");
    }

    #[test]
    fn collapses_duplicate_phrases() {
        assert_eq!(
            normalize_transcription("我我觉得这个这个功能可以"),
            "我觉得这个功能可以"
        );
        assert_eq!(normalize_transcription("然后然后我们开始"), "然后我们开始");
    }

    #[test]
    fn keeps_sentence_shape_conservative() {
        assert_eq!(
            normalize_transcription("  嗯  请帮我review一下这个PR  "),
            "嗯 请帮我review一下这个PR"
        );
    }

    #[test]
    fn streaming_preview_trims_leading_fillers() {
        assert_eq!(normalize_streaming_preview("嗯，帮我看一下"), "帮我看一下");
        assert_eq!(normalize_streaming_preview("呃 这个先别动"), "这个先别动");
    }

    #[test]
    fn streaming_rewrite_splits_and_punctuates() {
        assert_eq!(
            rewrite_streaming_text("嗯 帮我看一下这个pr 然后告诉我有没有风险"),
            vec!["帮我看一下这个pr，".to_string(), "然后告诉我有没有风险。".to_string()]
        );
    }

    #[test]
    fn streaming_rewrite_keeps_questions() {
        assert_eq!(
            rewrite_streaming_text("这个功能现在是不是已经可用了"),
            vec!["这个功能现在是不是已经可用了？".to_string()]
        );
    }
}
