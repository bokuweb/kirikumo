//! Finding things in a log.
//!
//! Deliberately *not* the fuzzy matcher the table and the palette use. A log
//! is searched for a literal — a request id, an IP, a word an exception
//! printed — and a fuzzy match over a hundred thousand lines finds every line
//! containing those letters in that order, which is every line. The thing
//! people already do to a log is `grep`, and this is that.
//!
//! Matching returns *indices*, so the log view can stay one virtualized list
//! over the lines the store holds rather than building a filtered copy of a
//! scrollback that may be fifty thousand lines long
//! (`AGENTS.md` rule 7).

/// The lines that contain `query`, case-insensitively, in order.
///
/// An empty query matches everything, which is how the box being empty shows
/// the whole log.
pub fn matching(lines: &[String], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..lines.len()).collect();
    }
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.to_lowercase().contains(&query))
        .map(|(index, _)| index)
        .collect()
}

/// How many lines contain `query`.
///
/// Cheaper than [`matching`] when only the count is wanted, which is what the
/// find box reports beside itself.
pub fn count(lines: &[String], query: &str) -> usize {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return lines.len();
    }
    lines
        .iter()
        .filter(|line| line.to_lowercase().contains(&query))
        .count()
}

/// How a variable-height virtual log should update its measured rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualListChange {
    /// Neither row identity nor content changed.
    Keep,
    /// This many rows were appended to the existing prefix.
    Append(usize),
    /// Existing row identities or contents changed and must be remeasured.
    Reset,
}

/// Reconcile the filtered rows with the last rendered virtual log.
///
/// A pure append preserves every existing height measurement and the
/// reader's scroll anchor. Search changes, removals, and a capped buffer that
/// replaced content without changing its length require a reset.
pub fn virtual_list_change(
    previous: &[usize],
    current: &[usize],
    content_changed_without_growth: bool,
) -> VirtualListChange {
    if content_changed_without_growth {
        return VirtualListChange::Reset;
    }
    if previous == current {
        return VirtualListChange::Keep;
    }
    if current.starts_with(previous) {
        return VirtualListChange::Append(current.len() - previous.len());
    }
    VirtualListChange::Reset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<String> {
        [
            "2026-09-07T09:14:02Z INFO  starting, version=1.4.0",
            "2026-09-07T09:15:41Z WARN  slow query 1.82s SELECT * FROM orders",
            "2026-09-07T09:16:02Z ERROR upstream timeout service=inventory",
            "2026-09-07T09:16:33Z INFO  GET /healthz 200",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn an_empty_query_is_the_whole_log() {
        assert_eq!(matching(&lines(), ""), vec![0, 1, 2, 3]);
        assert_eq!(matching(&lines(), "   "), vec![0, 1, 2, 3]);
        assert_eq!(count(&lines(), ""), 4);
    }

    #[test]
    fn a_word_finds_the_lines_it_is_in() {
        assert_eq!(matching(&lines(), "INFO"), vec![0, 3]);
        assert_eq!(count(&lines(), "INFO"), 2);
    }

    #[test]
    fn matching_ignores_case() {
        assert_eq!(matching(&lines(), "error"), vec![2]);
        assert_eq!(matching(&lines(), "ERROR"), vec![2]);
    }

    #[test]
    fn a_query_is_literal_and_not_fuzzy() {
        // Fuzzily, "sql" is in "slow query ... SELECT": s-l-q. Literally it
        // is in nothing, which is the answer a person searching a log wants.
        assert!(matching(&lines(), "sql").is_empty());
        // And a literal substring with punctuation in it works, which a
        // fuzzy matcher would also have found — but so would a hundred
        // thousand other lines.
        assert_eq!(matching(&lines(), "version=1.4.0"), vec![0]);
    }

    #[test]
    fn nothing_matching_is_no_lines_rather_than_all_of_them() {
        assert!(matching(&lines(), "zzz").is_empty());
        assert_eq!(count(&lines(), "zzz"), 0);
    }

    #[test]
    fn an_empty_log_answers_without_complaint() {
        assert!(matching(&[], "anything").is_empty());
        assert_eq!(count(&[], ""), 0);
    }

    #[test]
    fn a_growing_match_set_only_appends_virtual_rows() {
        assert_eq!(
            virtual_list_change(&[0, 2], &[0, 2, 4], false),
            VirtualListChange::Append(1)
        );
    }

    #[test]
    fn filtering_or_replacing_a_capped_log_resets_virtual_measurements() {
        assert_eq!(
            virtual_list_change(&[0, 2, 4], &[1, 3], false),
            VirtualListChange::Reset
        );
        assert_eq!(
            virtual_list_change(&[0, 1], &[0, 1], true),
            VirtualListChange::Reset
        );
    }

    #[test]
    fn an_unchanged_log_keeps_its_virtual_measurements() {
        assert_eq!(
            virtual_list_change(&[0, 1], &[0, 1], false),
            VirtualListChange::Keep
        );
    }
}
