//! Text handling that has to know about scripts written without spaces between words
//! (Chinese, Japanese, Korean): decoding legacy encodings, cutting passages, and how the
//! keyword index sees a passage.

use anyhow::{bail, Result};

/// Budget for one passage, in rough token units: one per Latin character, three per CJK
/// character, which carries about as much meaning as a short English word.
const CHUNK_UNITS: usize = 1200;
const OVERLAP_UNITS: usize = 150;
const CJK_WEIGHT: usize = 3;

/// Han, kana, bopomofo and Hangul.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x11FF       // Hangul Jamo
        | 0x2E80..=0x2FDF     // CJK and Kangxi radicals
        | 0x3005..=0x3007     // 々 〆 〇
        | 0x3040..=0x30FF     // Hiragana, Katakana
        | 0x3100..=0x312F     // Bopomofo
        | 0x3130..=0x318F     // Hangul compatibility Jamo
        | 0x31A0..=0x31BF     // Bopomofo extended
        | 0x31F0..=0x31FF     // Katakana phonetic extensions
        | 0x3400..=0x4DBF     // CJK extension A
        | 0x4E00..=0x9FFF     // CJK unified ideographs
        | 0xAC00..=0xD7AF     // Hangul syllables
        | 0xF900..=0xFAFF     // CJK compatibility ideographs
        | 0xFF66..=0xFF9F     // Halfwidth katakana
        | 0x20000..=0x323AF   // CJK extensions B to H
    )
}

/// Joins words recognised by OCR into a line. Windows OCR reports each Chinese or
/// Japanese character as a word of its own; those are put back together without spaces.
pub fn join_words(words: &[String]) -> String {
    let mut line = String::new();
    for word in words.iter().filter(|w| !w.is_empty()) {
        let joins_previous = line.chars().last().is_some_and(is_cjk) && word.chars().next().is_some_and(is_cjk);
        if !line.is_empty() && !joins_previous {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}

// ---------------------------------------------------------------------------
// Decoding

/// Decodes a text file: a byte order mark wins, then UTF-8, then the most likely legacy
/// encoding (Big5, GBK, Shift_JIS, EUC-KR, Windows code pages and so on).
pub fn decode(bytes: &[u8]) -> Result<String> {
    // UTF-16 text is full of zero bytes, so the BOM is checked before the binary test.
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        return Ok(encoding.decode_without_bom_handling(&bytes[bom_len..]).0.into_owned());
    }
    if bytes.iter().take(4096).any(|b| *b == 0) {
        bail!("binary file");
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_owned());
    }
    // A UTF-8 file with a few damaged bytes is still UTF-8. Legacy CJK text read as UTF-8
    // is almost all invalid, with only the odd byte pair that happens to be valid.
    let (mut multibyte, mut invalid) = (0usize, 0usize);
    for chunk in bytes.utf8_chunks() {
        multibyte += chunk.valid().chars().filter(|c| c.len_utf8() > 1).count();
        invalid += usize::from(!chunk.invalid().is_empty());
    }
    if multibyte >= 8 * invalid {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    Ok(encoding.decode_without_bom_handling(bytes).0.into_owned())
}

// ---------------------------------------------------------------------------
// Passages

fn weight(c: char) -> usize {
    if is_cjk(c) {
        CJK_WEIGHT
    } else {
        1
    }
}

/// Ends a sentence in CJK text. ASCII `.` `!` `?` are left out: they also appear inside
/// numbers, abbreviations and URLs, and English text breaks on whitespace as before.
fn ends_sentence(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '；' | '…' | '｡')
}

/// Closing marks that belong to the sentence before them, as in 「好。」
fn closes(c: char) -> bool {
    matches!(c, '」' | '』' | '）' | '】' | '〉' | '》' | '”' | '’' | ')' | '"')
}

fn ends_clause(c: char) -> bool {
    matches!(c, '，' | '、' | '：' | ',' | '､')
}

/// Splits into overlapping passages of about `CHUNK_UNITS`. A break goes, in order of
/// preference, after a CJK sentence end, at whitespace, or after a comma, as long as it
/// falls in the second half of the passage.
pub fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut units = 0;
        while end < chars.len() && units + weight(chars[end]) <= CHUNK_UNITS {
            units += weight(chars[end]);
            end += 1;
        }

        if end < chars.len() {
            let half = start + (end - start) / 2;
            let window = &chars[half..end];
            let after_sentence = window.iter().rposition(|c| ends_sentence(*c)).map(|p| {
                let mut b = half + p + 1;
                while b < end && closes(chars[b]) {
                    b += 1;
                }
                b
            });
            let at_space = || window.iter().rposition(|c| c.is_whitespace()).map(|p| half + p);
            let after_clause = || window.iter().rposition(|c| ends_clause(*c)).map(|p| half + p + 1);
            if let Some(b) = after_sentence.or_else(at_space).or_else(after_clause) {
                end = b;
            }
        }

        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if piece.chars().count() >= 20 || (out.is_empty() && !piece.is_empty()) {
            out.push(piece.to_string());
        }
        if end == chars.len() {
            break;
        }

        // The next passage starts about OVERLAP_UNITS back, but always moves forward.
        let mut next = end;
        let mut back = 0;
        while next > start + 1 && back < OVERLAP_UNITS {
            next -= 1;
            back += weight(chars[next]);
        }
        start = next;
    }
    out
}

/// Joins consecutive passages from `chunk_text` back into running text, dropping the part
/// each one repeats from the one before.
pub fn join_passages<'a>(passages: impl IntoIterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for next in passages {
        if out.is_empty() {
            out.push_str(next);
            continue;
        }
        // The longest start of `next` that `out` already ends with. Every character weighs
        // at least one unit, so the overlap is never longer than OVERLAP_UNITS characters
        // (plus the whitespace trimmed off the end of the passage before).
        let starts: Vec<usize> = next.char_indices().map(|(i, _)| i).chain([next.len()]).collect();
        let reach = starts.len().min(OVERLAP_UNITS + 2);
        let shared = (1..reach)
            .rev()
            .map(|k| starts[k])
            .find(|&end| {
                let head = next[..end].trim_end();
                head.chars().count() >= 8 && out.ends_with(head)
            })
            .map(|end| next[..end].trim_end().len());
        match shared {
            Some(len) => out.push_str(&next[len..]),
            None => {
                out.push('\n');
                out.push_str(next);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Keyword index

/// Character folding shared by the index and queries: full-width Latin letters, digits and
/// punctuation become ASCII, the ideographic space a plain space, and 臺 becomes 台.
fn fold(c: char) -> char {
    match c as u32 {
        0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        0x3000 => ' ',
        _ if c == '臺' => '台',
        _ => c,
    }
}

/// How the keyword index sees a passage. FTS5's tokenizer keeps a run of CJK characters
/// as a single token, so a two-character word inside a sentence could never match. Here
/// each CJK character becomes a token of its own, and a query asks for them as a phrase,
/// which matches the characters in that order anywhere in the text.
///
/// The index stores what this returns, so changing its output needs a new
/// `KEYWORD_INDEX` version in `db.rs`, which rebuilds the index.
pub fn fts_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 2);
    let mut prev_cjk = false;
    for c in text.chars().map(fold) {
        let cjk = is_cjk(c);
        if (cjk || prev_cjk) && !out.is_empty() {
            out.push(' ');
        }
        out.push(c);
        prev_cjk = cjk;
    }
    out
}

/// The FTS5 query for the exact-match side: each word typed becomes a quoted phrase, and
/// every one must appear in the passage. Empty when the query has no words.
pub fn fts_query(query: &str) -> String {
    let folded: String = query.chars().map(fold).collect();
    folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"", fts_text(w)))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_ocr_words() {
        let words = |ws: &[&str]| ws.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        assert_eq!(join_words(&words(&["今", "天", "是", "2024", "年", "Total:", "NT$", "300"])), "今天是 2024 年 Total: NT$ 300");
        assert_eq!(join_words(&words(&[])), "");
    }

    #[test]
    fn splits_cjk_into_single_tokens() {
        assert_eq!(fts_text("房租五千"), "房 租 五 千");
        assert_eq!(fts_text("iPhone手機2台"), "iPhone 手 機 2 台");
        assert_eq!(fts_text("hello world"), "hello world");
    }

    #[test]
    fn folds_full_width_and_variants() {
        assert_eq!(fts_text("ＡＢＣ１２３"), "ABC123");
        assert_eq!(fts_text("臺北"), "台 北");
        assert_eq!(fts_query("臺鐵　ＧＰＳ"), "\"台 鐵\" \"GPS\"");
    }

    #[test]
    fn builds_phrase_queries() {
        assert_eq!(fts_query("房租 多少"), "\"房 租\" \"多 少\"");
        assert_eq!(fts_query("invoice 2024-03"), "\"invoice\" \"2024\" \"03\"");
        assert_eq!(fts_query("  ?! "), "");
    }

    #[test]
    fn decodes_legacy_encodings() {
        let text = "這是一份租屋合約，每月房租新台幣一萬兩千元，押金兩個月，水電費另計。";
        let (big5, _, bad) = encoding_rs::BIG5.encode(text);
        assert!(!bad);
        assert_eq!(decode(&big5).unwrap(), text);

        let simplified = "这是一份租房合同，每月房租人民币三千元，押金两个月，水电费另计。";
        let (gbk, _, bad) = encoding_rs::GBK.encode(simplified);
        assert!(!bad);
        assert_eq!(decode(&gbk).unwrap(), simplified);

        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode(&utf16).unwrap(), text);

        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend(text.as_bytes());
        assert_eq!(decode(&bom).unwrap(), text);

        assert!(decode(b"\x00\x01\x02binary").is_err());
    }

    #[test]
    fn english_chunks_are_unchanged() {
        let words: String = (0..600).map(|i| format!("word{i} ")).collect();
        let chunks = chunk_text(&words);
        assert!(chunks.len() > 1);
        // Latin text still breaks at whitespace, within the same character budget.
        for c in &chunks {
            assert!(c.chars().count() <= CHUNK_UNITS);
            assert!(words.contains(&format!("{c} ")) || words.trim_end().ends_with(c.as_str()));
        }
    }

    #[test]
    fn cjk_chunks_break_after_sentences() {
        let sentence = "這是測試用的一句話，用來確認切段會停在句號之後。";
        let text = sentence.repeat(60);
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 1);
        for c in &chunks[..chunks.len() - 1] {
            assert!(c.ends_with('。'), "{c}");
            let units: usize = c.chars().map(weight).sum();
            assert!(units <= CHUNK_UNITS && units > CHUNK_UNITS / 2);
        }
        // Consecutive passages overlap.
        let tail: String = chunks[0].chars().skip(chunks[0].chars().count() - 10).collect();
        assert!(chunks[1].contains(&tail));
    }

    #[test]
    fn keeps_closing_quote_with_sentence() {
        let block = format!("{}「好。」{}", "字".repeat(300), "後".repeat(200));
        let first = &chunk_text(&block)[0];
        assert!(first.ends_with("。」"), "{first}");
    }

    #[test]
    fn passages_join_back_into_the_text() {
        let words: String = (0..900).map(|i| format!("word{i} ")).collect();
        let joined = join_passages(chunk_text(&words).iter().map(String::as_str));
        assert_eq!(joined, words.trim());

        let text: String = (0..80).map(|i| format!("這是第{i}句測試用的話，用來確認切段會停在句號之後。")).collect();
        let joined = join_passages(chunk_text(&text).iter().map(String::as_str));
        assert_eq!(joined, text);

        // Passages that do not overlap stay apart.
        assert_eq!(join_passages(["first part of it", "something else here"]), "first part of it\nsomething else here");
    }

    #[test]
    fn always_moves_forward() {
        let text = "字".repeat(5000);
        let chunks = chunk_text(&text);
        assert!(chunks.len() > 10 && chunks.len() < 20);
    }
}
