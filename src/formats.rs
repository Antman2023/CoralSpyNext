//! Portable export helpers. RTF preserves Unicode text, not provider styling.
pub fn text_to_rtf(text: &str) -> String {
    let mut out = String::from("{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Segoe UI;}}\\uc1\\f0\\fs20 ");
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for ch in normalized.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '\n' => out.push_str("\\par\n"),
            '\r' => {}
            '\t' => out.push_str("\\tab "),
            c if c.is_ascii() && !c.is_control() => out.push(c),
            c if !c.is_control() => {
                let mut units = [0; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
            _ => {}
        }
    }
    out.push('}');
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rtf_cr_and_crlf_preserved() {
        assert_eq!(text_to_rtf("a\rb"), text_to_rtf("a\r\nb"));
        assert!(text_to_rtf("a\rb").contains("a\\par\nb"));
    }
    #[test]
    fn rtf_escapes_markup() {
        assert!(text_to_rtf("{\\}").contains("\\{\\\\\\}"));
    }
    #[test]
    fn rtf_unicode_surrogates() {
        let s = text_to_rtf("你好🦀");
        assert!(s.contains("\\u20320?"));
        assert!(s.contains("\\u-10178?"));
    }
    #[test]
    fn rtf_linebreaks() {
        assert!(text_to_rtf("a\nb\tc").contains("a\\par\nb\\tab c"));
    }
}
