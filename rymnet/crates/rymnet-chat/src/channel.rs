use anyhow::{Result, bail};

/// `#name`: leading `#` required (classic IRC), followed by 1-32
/// lowercase letters/digits/`-`/`_`. Channels are their own namespace --
/// no claim, no ownership, unlike `.rym` domains or usernames -- so this
/// deliberately doesn't depend on `rymnet-account`; it's just a shape
/// check.
pub fn validate_channel(name: &str) -> Result<()> {
    let Some(tail) = name.strip_prefix('#') else {
        bail!("channel name must start with '#'");
    };
    let len_ok = (1..=32).contains(&tail.len());
    let chars_ok = tail
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if !len_ok || !chars_ok {
        bail!("channel name must be '#' followed by 1-32 lowercase letters, digits, '-' or '_'");
    }
    Ok(())
}

/// Filesystem-safe directory name for a channel -- the leading `#` is
/// meaningful on the wire and in the UI, not on disk. Always validates
/// first, so a caller can never turn an unchecked string into a path.
pub fn channel_dir_name(name: &str) -> Result<String> {
    validate_channel(name)?;
    Ok(name.trim_start_matches('#').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_names() {
        assert!(validate_channel("#general").is_ok());
        assert!(validate_channel("#rym-net_2").is_ok());
        assert_eq!(channel_dir_name("#general").unwrap(), "general");
    }

    #[test]
    fn rejects_missing_hash() {
        assert!(validate_channel("general").is_err());
    }

    #[test]
    fn rejects_bad_characters_and_path_traversal() {
        assert!(validate_channel("#..").is_err());
        assert!(validate_channel("#a/b").is_err());
        assert!(validate_channel("#Upper").is_err());
        assert!(validate_channel("# space").is_err());
        assert!(channel_dir_name("#../../etc").is_err());
    }

    #[test]
    fn rejects_empty_and_too_long() {
        assert!(validate_channel("#").is_err());
        assert!(validate_channel(&format!("#{}", "a".repeat(33))).is_err());
        assert!(validate_channel(&format!("#{}", "a".repeat(32))).is_ok());
    }
}
