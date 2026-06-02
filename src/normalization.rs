// Unicode normalization for cross-platform consistency.
//
// FileMaker on macOS tends to emit accented characters in decomposed form
// (NFD: "e" + combining acute = U+0065 U+0301), while Windows emits the
// precomposed form (NFC: "é" = U+00E9). They render identically but are
// distinct byte sequences, so a name copied on one platform won't string-match
// the same name copied on the other. We normalize everything to NFC when
// ingesting FM data so the .fmscript output is identical regardless of origin.
//
// On Windows this is a no-op in practice: input is already NFC, so to_nfc is
// idempotent and existing behavior is unchanged.

use unicode_normalization::UnicodeNormalization;

/// Normalize a string to NFC (precomposed form).
/// Applied once to the decoded FM XML so all names, calculations, and comments
/// share a single canonical form.
pub fn to_nfc(s: &str) -> String {
    s.nfc().collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfc_preserves_ascii() {
        assert_eq!(to_nfc("Hello World"), "Hello World");
    }

    #[test]
    fn nfc_composes_accented() {
        // NFD: "e" + combining acute accent → NFC: precomposed "é"
        assert_eq!(to_nfc("e\u{0301}"), "\u{00E9}");
    }

    #[test]
    fn nfc_is_idempotent_on_precomposed() {
        // Windows-style precomposed input must pass through unchanged.
        let nfc = "cañón";
        assert_eq!(to_nfc(nfc), nfc);
    }

    #[test]
    fn nfc_handles_multiple_combining_marks() {
        // "s" + dot-below (U+0323) + dot-above (U+0307) → NFC composes to ṩ.
        let decomposed = "s\u{0323}\u{0307}";
        assert_eq!(to_nfc(decomposed), "\u{1E69}");
    }
}
