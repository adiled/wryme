#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proword {
    Cls,
}

pub struct Spec {
    pub word: &'static str,
    pub proword: Proword,
    pub about: &'static str,
}

pub const PROWORDS: &[Spec] = &[Spec {
    word: "cls",
    proword: Proword::Cls,
    about: "file this stretch, clear the screen, carry on from that page",
}];

pub fn parse(text: &str) -> Option<Result<Proword, String>> {
    let text = text.trim();
    let word = text.strip_prefix('/')?.split_whitespace().next().unwrap_or("");
    if !word.chars().all(|c| c.is_ascii_alphabetic()) || std::path::Path::new(text).exists() {
        return None;
    }
    let word = word.to_ascii_lowercase();
    Some(
        PROWORDS
            .iter()
            .find(|s| s.word == word)
            .map(|s| s.proword)
            .ok_or(word),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_words_are_prowords_but_paths_are_not() {
        assert_eq!(parse("/cls"), Some(Ok(Proword::Cls)));
        assert_eq!(parse("  /CLS  "), Some(Ok(Proword::Cls)));
        assert_eq!(parse("/nope"), Some(Err("nope".into())));
        assert_eq!(parse("/Users/adil/shot.png"), None);
        assert_eq!(parse("/tmp"), None);
        assert_eq!(parse("hello /cls"), None);
        assert_eq!(parse("/2fa code"), None);
    }
}
