use anyhow::{Result, bail};

/// A board name: 1-32 lowercase letters/digits/`-`/`_`, no leading
/// symbol required -- Reddit-style (`rust`, `askrymnet`), not IRC-style
/// like chat's `#channel`. Boards are their own namespace -- no claim, no
/// ownership, same posture as chat channels -- so this deliberately
/// doesn't depend on `rymnet-account`; it's just a shape check.
pub fn validate_board(name: &str) -> Result<()> {
    let len_ok = (1..=32).contains(&name.len());
    let chars_ok = name.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if !len_ok || !chars_ok {
        bail!("board name must be 1-32 lowercase letters, digits, '-' or '_'");
    }
    Ok(())
}

/// Filesystem-safe directory name for a board. Always validates first,
/// so a caller can never turn an unchecked string into a path -- since a
/// board name has no leading symbol to strip, this is really just a
/// validating identity function, kept as its own step to mirror
/// `rymnet_chat::channel_dir_name`'s shape (and so callers never have to
/// know that boards and channels differ here).
pub fn board_dir_name(name: &str) -> Result<String> {
    validate_board(name)?;
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_names() {
        assert!(validate_board("rust").is_ok());
        assert!(validate_board("rym-net_2").is_ok());
        assert_eq!(board_dir_name("askrymnet").unwrap(), "askrymnet");
    }

    #[test]
    fn rejects_bad_characters_and_path_traversal() {
        assert!(validate_board("..").is_err());
        assert!(validate_board("a/b").is_err());
        assert!(validate_board("Upper").is_err());
        assert!(validate_board(" space").is_err());
        assert!(board_dir_name("../../etc").is_err());
    }

    #[test]
    fn rejects_empty_and_too_long() {
        assert!(validate_board("").is_err());
        assert!(validate_board(&"a".repeat(33)).is_err());
        assert!(validate_board(&"a".repeat(32)).is_ok());
    }
}
