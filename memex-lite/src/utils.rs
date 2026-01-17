//! 工具函数

/// UTF-8 安全的字符串截断
///
/// 按字符数截断，而不是字节数，避免中文等多字节字符被截断
pub fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars].iter().collect();
        format!("{}...", truncated)
    }
}

/// UTF-8 安全的字符串截断（带自定义后缀）
#[allow(dead_code)]
pub fn truncate_str_with_suffix(s: &str, max_chars: usize, suffix: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars].iter().collect();
        format!("{}{}", truncated, suffix)
    }
}

/// UTF-8 安全的子字符串提取
///
/// 按字符索引提取，返回 (start_byte, end_byte) 用于切片
pub fn char_range_to_byte_range(s: &str, start_char: usize, end_char: usize) -> (usize, usize) {
    let char_indices: Vec<(usize, char)> = s.char_indices().collect();

    let start_byte = char_indices
        .get(start_char)
        .map(|(i, _)| *i)
        .unwrap_or(0);
    let end_byte = char_indices
        .get(end_char)
        .map(|(i, _)| *i)
        .unwrap_or(s.len());

    (start_byte, end_byte.min(s.len()))
}

/// UTF-8 安全的子字符串提取（直接返回字符串）
pub fn substr_by_chars(s: &str, start_char: usize, end_char: usize) -> &str {
    let (start_byte, end_byte) = char_range_to_byte_range(s, start_char, end_char);
    &s[start_byte..end_byte]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello...");
        assert_eq!(truncate_str("hello", 5), "hello");
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_chinese() {
        // 中文字符
        assert_eq!(truncate_str("你好世界", 2), "你好...");
        assert_eq!(truncate_str("你好", 2), "你好");
        assert_eq!(truncate_str("你好", 10), "你好");
    }

    #[test]
    fn test_truncate_mixed() {
        // 中英混合
        assert_eq!(truncate_str("hello你好", 7), "hello你好");
        assert_eq!(truncate_str("hello你好world", 7), "hello你好...");
        assert_eq!(truncate_str("你好hello世界", 4), "你好he...");
    }

    #[test]
    fn test_truncate_emoji() {
        // Emoji（多字节）
        assert_eq!(truncate_str("👋🌍🎉", 2), "👋🌍...");
        assert_eq!(truncate_str("hi👋", 3), "hi👋");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate_str("", 5), "");
        assert_eq!(truncate_str("", 0), "");
    }

    #[test]
    fn test_substr_by_chars_chinese() {
        let s = "你好世界hello";
        assert_eq!(substr_by_chars(s, 0, 2), "你好");
        assert_eq!(substr_by_chars(s, 2, 4), "世界");
        assert_eq!(substr_by_chars(s, 4, 9), "hello");
    }

    #[test]
    fn test_substr_by_chars_boundary() {
        let s = "abc中文def";
        // 不会在中文字符中间截断
        assert_eq!(substr_by_chars(s, 0, 3), "abc");
        assert_eq!(substr_by_chars(s, 3, 5), "中文");
        assert_eq!(substr_by_chars(s, 5, 8), "def");
    }

    #[test]
    fn test_char_range_to_byte_range() {
        let s = "a中b";
        // 'a' = 1 byte, '中' = 3 bytes, 'b' = 1 byte
        assert_eq!(char_range_to_byte_range(s, 0, 1), (0, 1));  // "a"
        assert_eq!(char_range_to_byte_range(s, 1, 2), (1, 4));  // "中"
        assert_eq!(char_range_to_byte_range(s, 2, 3), (4, 5));  // "b"
        assert_eq!(char_range_to_byte_range(s, 0, 3), (0, 5));  // "a中b"
    }
}
