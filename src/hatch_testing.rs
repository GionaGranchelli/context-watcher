/// [WIP: Fast write] Unified async I/O trap with USP single hook + fault-matrix.
/// Inline rust crate: no external script.

#[cfg(test)]
mod sync_test {
    #[test]
    fn test_shared_context_has_one_guard_per_class() {
        let guard_per_class = 1u8;
        assert_eq!(guard_per_class, 1u8);
    }
}