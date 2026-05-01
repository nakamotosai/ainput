const DUPLICATE_PHRASES: &[&str] = &[
    "我", "我们", "你", "你们", "他", "她", "这个", "那个", "就是", "然后", "所以", "可以",
];
const STREAMING_FILLER_PREFIXES: &[&str] = &["嗯", "呃", "额", "啊"];
const STREAMING_SEGMENT_MIN_CHARS: usize = 6;
const STREAMING_SEGMENT_HARD_LIMIT: usize = 28;
const COMMON_CN_COMMA_BEFORE_MARKERS: &[(&str, usize)] = &[
    ("但是", 4),
    ("不过", 4),
    ("只是", 4),
    ("所以", 4),
    ("因此", 4),
    ("而且", 4),
    ("并且", 4),
    ("然后", 6),
    ("现在", 4),
];
const COMMON_PRODUCT_ALIASES: &[(&str, &str)] = &[
    ("hud", "HUD"),
    ("hot", "HUD"),
    ("ht", "HUD"),
    ("hut", "HUD"),
    ("gpt4o", "GPT-4o"),
    ("gpt 4o", "GPT-4o"),
    ("codex cli", "Codex CLI"),
    ("condex cli", "Codex CLI"),
    ("codex ci", "Codex CI"),
    ("condex ci", "Codex CI"),
    ("claude ops", "Claude Opus"),
    ("cloud ops", "Claude Opus"),
    ("claude opus", "Claude Opus"),
    ("cloud opus", "Claude Opus"),
    ("google gemini", "Google Gemini"),
    ("google germany", "Google Gemini"),
];
const COMMON_TEXT_REWRITES: &[(&str, &str)] = &[("证确", "正确"), ("土字", "吐字")];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestSentenceRewrite {
    pub frozen_prefix: String,
    pub original_sentence: String,
    pub rewritten_sentence: String,
}

impl LatestSentenceRewrite {
    pub fn combined_text(&self) -> String {
        format!("{}{}", self.frozen_prefix, self.rewritten_sentence)
            .trim()
            .to_string()
    }
}

pub fn normalize_transcription(text: &str) -> String {
    let mut current = collapse_whitespace(text);
    current = collapse_known_duplicates(&current);
    current = cleanup_punctuation_spacing(&current);
    current = normalize_chinese_number_sequences(&current);
    current = merge_spelled_ascii_sequences(&current);
    current = normalize_common_product_names(&current);
    current = normalize_common_text_rewrites(&current);

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

pub fn rewrite_latest_sentence(text: &str) -> LatestSentenceRewrite {
    rewrite_latest_sentence_with_mode(text, true)
}

pub fn rewrite_latest_sentence_preview(text: &str) -> LatestSentenceRewrite {
    rewrite_latest_sentence_with_mode(text, false)
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

fn merge_spelled_ascii_sequences(text: &str) -> String {
    let tokens: Vec<&str> = text.split(' ').collect();
    if tokens.len() <= 1 {
        return text.to_string();
    }

    let mut merged: Vec<String> = Vec::with_capacity(tokens.len());
    let mut index = 0usize;

    while index < tokens.len() {
        let token = tokens[index];
        if !is_single_spelling_token(token) {
            merged.push(token.to_string());
            index += 1;
            continue;
        }

        let mut end = index + 1;
        while end < tokens.len() && is_single_spelling_token(tokens[end]) {
            end += 1;
        }

        if end - index >= 2 {
            let mut combined = String::new();
            for part in &tokens[index..end] {
                combined.push_str(&normalize_spelling_token(part));
            }
            merged.push(combined);
            index = end;
            continue;
        }

        merged.push(token.to_string());
        index += 1;
    }

    merged.join(" ")
}

fn normalize_chinese_number_sequences(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < chars.len() {
        if !is_chinese_number_char(chars[index]) {
            result.push(chars[index]);
            index += 1;
            continue;
        }

        let start = index;
        while index < chars.len() && is_chinese_number_char(chars[index]) {
            index += 1;
        }

        let segment: String = chars[start..index].iter().collect();
        let previous = start.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let next = chars.get(index).copied();
        if should_normalize_chinese_number_segment(&segment, previous, next) {
            if let Some(normalized) = normalize_chinese_number_segment(&segment) {
                result.push_str(&normalized);
                continue;
            }
        }

        result.push_str(&segment);
    }

    result
}

fn should_normalize_chinese_number_segment(
    segment: &str,
    previous: Option<char>,
    next: Option<char>,
) -> bool {
    if previous == Some('第') {
        return false;
    }

    let char_count = segment.chars().count();
    if segment.chars().any(is_chinese_unit_char) {
        return char_count >= 2;
    }

    if char_count >= 2 {
        return true;
    }

    previous.is_some_and(is_ascii_term_char) && next.is_some_and(is_ascii_term_char)
}

fn normalize_chinese_number_segment(segment: &str) -> Option<String> {
    if segment.is_empty() {
        return None;
    }

    if segment.chars().all(is_chinese_digit_char) {
        let mut mapped = String::with_capacity(segment.len());
        for ch in segment.chars() {
            mapped.push(char::from(b'0' + chinese_digit_value(ch)? as u8));
        }
        return Some(mapped);
    }

    if !segment.chars().all(is_chinese_number_char) || !segment.chars().any(is_chinese_unit_char) {
        return None;
    }

    parse_chinese_number_with_units(segment).map(|value| value.to_string())
}

fn parse_chinese_number_with_units(segment: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut section = 0u64;
    let mut number = 0u64;
    let mut saw_number = false;

    for ch in segment.chars() {
        if let Some(digit) = chinese_digit_value(ch) {
            number = digit as u64;
            saw_number = true;
            continue;
        }

        match ch {
            '十' => {
                let value = if saw_number { number } else { 1 };
                section += value * 10;
                number = 0;
                saw_number = false;
            }
            '百' => {
                let value = if saw_number { number } else { 1 };
                section += value * 100;
                number = 0;
                saw_number = false;
            }
            '千' => {
                let value = if saw_number { number } else { 1 };
                section += value * 1000;
                number = 0;
                saw_number = false;
            }
            '万' => {
                section += number;
                if section == 0 {
                    section = 1;
                }
                total += section * 10_000;
                section = 0;
                number = 0;
                saw_number = false;
            }
            '亿' => {
                section += number;
                if section == 0 {
                    section = 1;
                }
                total += section * 100_000_000;
                section = 0;
                number = 0;
                saw_number = false;
            }
            _ => return None,
        }
    }

    Some(total + section + number)
}

fn chinese_digit_value(ch: char) -> Option<u32> {
    match ch {
        '零' | '〇' | '○' | '洞' => Some(0),
        '一' | '幺' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

fn is_chinese_digit_char(ch: char) -> bool {
    chinese_digit_value(ch).is_some()
}

fn is_chinese_unit_char(ch: char) -> bool {
    matches!(ch, '十' | '百' | '千' | '万' | '亿')
}

fn is_chinese_number_char(ch: char) -> bool {
    is_chinese_digit_char(ch) || is_chinese_unit_char(ch)
}

fn normalize_common_product_names(text: &str) -> String {
    let mut current = text.to_string();
    for (spoken, canonical) in COMMON_PRODUCT_ALIASES {
        current = replace_case_insensitive_ascii_phrase(&current, spoken, canonical);
    }
    current
}

fn normalize_common_text_rewrites(text: &str) -> String {
    let mut current = text.to_string();
    for (spoken, canonical) in COMMON_TEXT_REWRITES {
        current = current.replace(spoken, canonical);
    }
    current
}

fn replace_case_insensitive_ascii_phrase(text: &str, needle: &str, replacement: &str) -> String {
    let lower_text = text.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut byte_index = 0usize;

    while byte_index < text.len() {
        let remaining = &lower_text[byte_index..];
        if remaining.starts_with(&lower_needle)
            && ascii_phrase_boundary_ok(&chars, text, byte_index, needle.len())
        {
            result.push_str(replacement);
            byte_index += needle.len();
            continue;
        }

        let next_char = text[byte_index..]
            .chars()
            .next()
            .expect("current byte index should be on char boundary");
        result.push(next_char);
        byte_index += next_char.len_utf8();
    }

    result
}

fn ascii_phrase_boundary_ok(chars: &[char], text: &str, start: usize, len: usize) -> bool {
    let start_char_index = text[..start].chars().count();
    let end_char_index = text[..start + len].chars().count();
    let before = start_char_index
        .checked_sub(1)
        .and_then(|index| chars.get(index))
        .copied();
    let after = chars.get(end_char_index).copied();

    before.map(|ch| !is_ascii_term_char(ch)).unwrap_or(true)
        && after.map(|ch| !is_ascii_term_char(ch)).unwrap_or(true)
}

fn is_ascii_term_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '+')
}

fn is_single_spelling_token(token: &str) -> bool {
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => ch.is_ascii_alphanumeric() || is_chinese_digit_char(ch),
        _ => false,
    }
}

fn normalize_spelling_token(token: &str) -> String {
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if is_chinese_digit_char(ch) => chinese_digit_value(ch)
            .map(|digit| char::from(b'0' + digit as u8).to_string())
            .unwrap_or_else(|| token.to_string()),
        _ => token.to_string(),
    }
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
    if matches!(
        next,
        ' ' | ',' | '，' | '.' | '。' | '!' | '！' | '?' | '？'
    ) {
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

fn find_segment_boundary(chars: &[char], start: usize, hard_end: usize) -> Option<usize> {
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

fn rewrite_latest_sentence_with_mode(text: &str, is_final: bool) -> LatestSentenceRewrite {
    let normalized = normalize_transcription(text);
    let (frozen_prefix, original_sentence) = split_latest_sentence(&normalized);
    if original_sentence.trim().is_empty() {
        return LatestSentenceRewrite {
            frozen_prefix,
            original_sentence,
            rewritten_sentence: String::new(),
        };
    }

    let mut rewritten_sentence = normalize_streaming_preview(&original_sentence);
    rewritten_sentence = apply_common_cn_comma_boundaries(&rewritten_sentence);
    if !rewritten_sentence.is_empty() && !has_terminal_punctuation(&rewritten_sentence) {
        rewritten_sentence.push(infer_streaming_terminal_punctuation(
            &rewritten_sentence,
            is_final,
        ));
    }

    LatestSentenceRewrite {
        frozen_prefix,
        original_sentence,
        rewritten_sentence,
    }
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

fn apply_common_cn_comma_boundaries(text: &str) -> String {
    let mut current = text.to_string();
    for (marker, min_prefix_chars) in COMMON_CN_COMMA_BEFORE_MARKERS {
        current = insert_boundary_before_marker(&current, marker, '，', *min_prefix_chars);
    }
    cleanup_punctuation_spacing(&current)
}

fn insert_boundary_before_marker(
    text: &str,
    marker: &str,
    boundary: char,
    min_prefix_chars: usize,
) -> String {
    if text.is_empty() || marker.is_empty() {
        return text.to_string();
    }

    let mut output = String::with_capacity(text.len() + 8);
    let mut cursor = 0usize;

    while let Some(relative_index) = text[cursor..].find(marker) {
        let index = cursor + relative_index;
        output.push_str(&text[cursor..index]);
        if should_insert_boundary_before(text, index, min_prefix_chars) {
            output.push(boundary);
        }
        output.push_str(marker);
        cursor = index + marker.len();
    }

    output.push_str(&text[cursor..]);
    output
}

fn should_insert_boundary_before(text: &str, index: usize, min_prefix_chars: usize) -> bool {
    let prefix = text[..index].trim_end();
    if prefix.is_empty() {
        return false;
    }
    if prefix.chars().count() < min_prefix_chars {
        return false;
    }

    let previous = match prefix.chars().last() {
        Some(previous) => previous,
        None => return false,
    };

    !matches!(
        previous,
        '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';'
    )
}

fn has_terminal_punctuation(text: &str) -> bool {
    text.chars().last().is_some_and(|ch| {
        matches!(
            ch,
            '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';'
        )
    })
}

fn split_latest_sentence(text: &str) -> (String, String) {
    let mut committed_end = 0usize;
    let mut trailing_after_boundary = false;

    for (index, ch) in text.char_indices() {
        let char_end = index + ch.len_utf8();
        if is_strong_boundary(ch) {
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

    let (frozen_prefix, latest_sentence) = text.split_at(committed_end);
    (
        frozen_prefix.to_string(),
        latest_sentence.trim().to_string(),
    )
}

fn is_strong_boundary(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。' | '！' | '？' | '；' | ',' | '.' | '!' | '?' | ';'
    )
}

fn is_soft_boundary(ch: char) -> bool {
    is_strong_boundary(ch) || is_soft_separator(ch)
}

fn is_soft_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '、' | '/' | '|' | '-' | '—')
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
    use super::{
        normalize_streaming_preview, normalize_transcription, rewrite_latest_sentence,
        rewrite_latest_sentence_preview, rewrite_streaming_text,
    };

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
    fn normalize_transcription_merges_spelled_ascii_sequences() {
        assert_eq!(normalize_transcription("h u d 上面"), "HUD 上面");
        assert_eq!(normalize_transcription("P R review"), "PR review");
        assert_eq!(normalize_transcription("g p t 4 o 模式"), "GPT-4o 模式");
        assert_eq!(normalize_transcription("open ai"), "open ai");
    }

    #[test]
    fn normalize_transcription_converts_chinese_numbers_to_arabic_digits() {
        assert_eq!(
            normalize_transcription("验证码一二三四五六"),
            "验证码123456"
        );
        assert_eq!(normalize_transcription("今年是二零二六年"), "今年是2026年");
        assert_eq!(normalize_transcription("一百二十三个"), "123个");
        assert_eq!(normalize_transcription("g p t 四 o 模式"), "GPT-4o 模式");
    }

    #[test]
    fn normalize_transcription_fixes_common_product_name_mishears() {
        assert_eq!(normalize_transcription("condex cli"), "Codex CLI");
        assert_eq!(normalize_transcription("cloud ops"), "Claude Opus");
        assert_eq!(normalize_transcription("google germany"), "Google Gemini");
        assert_eq!(normalize_transcription("codex ci"), "Codex CI");
        assert_eq!(normalize_transcription("hot 上面"), "HUD 上面");
        assert_eq!(normalize_transcription("hut 上面"), "HUD 上面");
    }

    #[test]
    fn streaming_rewrite_splits_and_punctuates() {
        assert_eq!(
            rewrite_streaming_text("嗯 帮我看一下这个pr 然后告诉我有没有风险"),
            vec![
                "帮我看一下这个pr，".to_string(),
                "然后告诉我有没有风险。".to_string()
            ]
        );
    }

    #[test]
    fn streaming_rewrite_keeps_questions() {
        assert_eq!(
            rewrite_streaming_text("这个功能现在是不是已经可用了"),
            vec!["这个功能现在是不是已经可用了？".to_string()]
        );
    }

    #[test]
    fn rewrite_latest_sentence_keeps_prefix_untouched() {
        let rewritten = rewrite_latest_sentence("第一句已经稳定。嗯 第二句有点乱");
        assert_eq!(rewritten.frozen_prefix, "第一句已经稳定。");
        assert_eq!(rewritten.rewritten_sentence, "第二句有点乱。");
        assert_eq!(rewritten.combined_text(), "第一句已经稳定。第二句有点乱。");
    }

    #[test]
    fn rewrite_latest_sentence_preview_does_not_rewrite_prefix() {
        let rewritten = rewrite_latest_sentence_preview("第一句已经稳定。呃 第二句先是错字");
        assert_eq!(rewritten.frozen_prefix, "第一句已经稳定。");
        assert_eq!(rewritten.rewritten_sentence, "第二句先是错字，");
        assert_eq!(
            rewritten.combined_text(),
            "第一句已经稳定。第二句先是错字，"
        );
    }

    #[test]
    fn normalize_transcription_fixes_common_streaming_mishears() {
        assert_eq!(
            normalize_transcription("明明这个hot上面已经把证确的文案显示出来了"),
            "明明这个HUD上面已经把正确的文案显示出来了"
        );
    }

    #[test]
    fn rewrite_latest_sentence_does_not_apply_project_specific_phrase_rewrites() {
        let rewritten = rewrite_latest_sentence(
            "然后不管我说多少个字他永远只能显示出来两个字应该是我不断地说话之后他能不断地出现文字明明这个哈上面已经把正确的文案显示出来了但是他有时候上评还是慢",
        );
        assert_eq!(
            rewritten.combined_text(),
            "然后不管我说多少个字他永远只能显示出来两个字应该是我不断地说话之后他能不断地出现文字明明这个哈上面已经把正确的文案显示出来了，但是他有时候上评还是慢。"
        );
    }

    #[test]
    fn rewrite_latest_sentence_inserts_generic_comma_before_connectors() {
        let rewritten = rewrite_latest_sentence("我想试一下这个功能然后再告诉你结果");
        assert_eq!(
            rewritten.combined_text(),
            "我想试一下这个功能，然后再告诉你结果。"
        );
    }

    #[test]
    fn rewrite_latest_sentence_inserts_generic_comma_before_now_clause() {
        let rewritten = rewrite_latest_sentence("我的名字叫老蔡现在这个土字不够丝滑");
        assert_eq!(
            rewritten.combined_text(),
            "我的名字叫老蔡，现在这个吐字不够丝滑。"
        );
    }
}
