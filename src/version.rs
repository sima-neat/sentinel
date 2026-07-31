pub const VERSION: &str = match option_env!("SENTINEL_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_version_is_never_empty() {
        assert!(!VERSION.is_empty());
    }
}
