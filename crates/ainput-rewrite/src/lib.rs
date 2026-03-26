const LEADING_FILLERS: &[&str] = &[
    "嗯嗯",
    "嗯",
    "啊啊",
    "啊",
    "呃呃",
    "呃",
    "额额",
    "额",
    "那个那个",
    "就是就是",
];

const DUPLICATE_PHRASES: &[&str] = &[
    "我", "我们", "你", "你们", "他", "她", "这个", "那个", "就是", "然后", "所以", "可以",
];

pub fn normalize_transcription(text: &str) -> String {
    let mut current = collapse_whitespace(text);
    current = strip_leading_fillers(&current);
    current = collapse_known_duplicates(&current);
    current = cleanup_punctuation_spacing(&current);

    current.trim().to_string()
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

fn strip_leading_fillers(text: &str) -> String {
    let mut current = text.trim_start().to_string();

    loop {
        let mut removed = false;
        for filler in LEADING_FILLERS {
            if current.starts_with(filler) {
                let remainder = current[filler.len()..].trim_start_matches(is_soft_separator);
                current = remainder.trim_start().to_string();
                removed = true;
                break;
            }
        }

        if !removed {
            break;
        }
    }

    current
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

fn is_soft_separator(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '，' | ',' | '。' | '.' | '！' | '？')
}

fn is_cjk_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。' | '！' | '？' | '：' | '；' | '、' | '）' | '】' | '》'
    )
}

#[cfg(test)]
mod tests {
    use super::normalize_transcription;

    #[test]
    fn removes_leading_fillers() {
        assert_eq!(normalize_transcription("嗯，帮我看一下"), "帮我看一下");
        assert_eq!(normalize_transcription("那个那个这个问题"), "这个问题");
        assert_eq!(normalize_transcription("就是就是这个问题"), "这个问题");
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
            "请帮我review一下这个PR"
        );
    }
}
