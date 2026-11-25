//! Bounds for quick search (plan W5.1). The admin pushes values within these bounds to `model-gateway` and the server,
//! and each refuses a value outside them. A query is capped at the MCP front's own limit (`search` takes at most 200
//! characters), so a longer one could never be searched.

/// Queries one reply may search with. Three covers a question with two angles; five is the most a push may ask for.
pub const MIN_QUERIES: u32 = 1;
pub const DEFAULT_QUERIES: u32 = 3;
pub const MAX_QUERIES: u32 = 5;

/// Characters one query may have. The MCP front's `search` refuses longer, so this cannot be raised above 200.
pub const MIN_QUERY_CHARS: u32 = 20;
pub const DEFAULT_QUERY_CHARS: u32 = 200;
pub const MAX_QUERY_CHARS: u32 = 200;

/// Pages one reply may fetch, all from its search results. Three keeps a reply to a few seconds of reading.
pub const MIN_PAGES: u32 = 1;
pub const DEFAULT_PAGES: u32 = 3;
pub const MAX_PAGES: u32 = 5;

/// Searches one user may start in an hour. Ten is a few questions' worth; a hundred is the most a push may ask for.
pub const MIN_SEARCHES_PER_HOUR: u32 = 1;
pub const DEFAULT_SEARCHES_PER_HOUR: u32 = 10;
pub const MAX_SEARCHES_PER_HOUR: u32 = 100;

/// The four quick-search limits as they are pushed. The default is the `DEFAULT_*` values, so a service that has not
/// been pushed one runs on the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuickSearchLimits {
    pub max_queries: u32,
    pub max_query_chars: u32,
    pub max_pages: u32,
    pub searches_per_user_hour: u32,
}

impl QuickSearchLimits {
    pub const DEFAULT: QuickSearchLimits = QuickSearchLimits {
        max_queries: DEFAULT_QUERIES,
        max_query_chars: DEFAULT_QUERY_CHARS,
        max_pages: DEFAULT_PAGES,
        searches_per_user_hour: DEFAULT_SEARCHES_PER_HOUR,
    };

    /// Every value within its bounds. The message names no value.
    pub fn validate(&self) -> Result<(), &'static str> {
        let ok = (MIN_QUERIES..=MAX_QUERIES).contains(&self.max_queries)
            && (MIN_QUERY_CHARS..=MAX_QUERY_CHARS).contains(&self.max_query_chars)
            && (MIN_PAGES..=MAX_PAGES).contains(&self.max_pages)
            && (MIN_SEARCHES_PER_HOUR..=MAX_SEARCHES_PER_HOUR).contains(&self.searches_per_user_hour);
        if ok {
            Ok(())
        } else {
            Err("A quick search limit is outside its bounds (askbar-proto, quick_search).")
        }
    }
}

impl Default for QuickSearchLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_default_sits_within_its_bounds() {
        let bounds = [
            (MIN_QUERIES, DEFAULT_QUERIES, MAX_QUERIES),
            (MIN_QUERY_CHARS, DEFAULT_QUERY_CHARS, MAX_QUERY_CHARS),
            (MIN_PAGES, DEFAULT_PAGES, MAX_PAGES),
            (MIN_SEARCHES_PER_HOUR, DEFAULT_SEARCHES_PER_HOUR, MAX_SEARCHES_PER_HOUR),
        ];
        for (min, default, max) in bounds {
            assert!(min <= default && default <= max, "{min} <= {default} <= {max}");
        }
        assert!(QuickSearchLimits::DEFAULT.validate().is_ok());
        assert_eq!(QuickSearchLimits::default(), QuickSearchLimits::DEFAULT);
    }

    #[test]
    fn a_value_outside_its_bounds_is_refused() {
        let d = QuickSearchLimits::DEFAULT;
        assert!(QuickSearchLimits { max_queries: MAX_QUERIES + 1, ..d }.validate().is_err());
        assert!(QuickSearchLimits { max_query_chars: MIN_QUERY_CHARS - 1, ..d }.validate().is_err());
        assert!(QuickSearchLimits { max_pages: 0, ..d }.validate().is_err());
        assert!(QuickSearchLimits { searches_per_user_hour: MAX_SEARCHES_PER_HOUR + 1, ..d }.validate().is_err());
    }
}
