//! Per-user tool permissions from the pushed config. Off by default (ADR 0019): a user with no entry holds nothing, and
//! a tool in `denied` is refused even if `allowed` lists it (deny wins).

use askbar_proto::mcp::{MAX_PERMISSION_USERS, MAX_PERMISSION_USER_LEN, TOOLS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserTools {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub denied: Vec<String>,
}

/// User ID to the tools that user holds.
pub type Permissions = BTreeMap<String, UserTools>;

pub fn holds(p: &Permissions, user: &str, tool: &str) -> bool {
    p.get(user).is_some_and(|u| u.allowed.iter().any(|t| t == tool) && !u.denied.iter().any(|t| t == tool))
}

/// The built-in tools `user` holds, in registry order.
pub fn permitted(p: &Permissions, user: &str) -> Vec<&'static str> {
    TOOLS.into_iter().filter(|t| holds(p, user, t)).collect()
}

/// Bounded, and every name is a tool the front has: a typo is refused at the push rather than silently holding nothing.
pub fn valid(p: &Permissions) -> bool {
    p.len() <= MAX_PERMISSION_USERS
        && p.iter().all(|(user, u)| {
            !user.is_empty()
                && user.len() <= MAX_PERMISSION_USER_LEN
                && u.allowed.iter().chain(&u.denied).all(|t| TOOLS.contains(&t.as_str()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_and_deny_wins() {
        let ut = |a: &[&str], d: &[&str]| UserTools {
            allowed: a.iter().map(|s| s.to_string()).collect(),
            denied: d.iter().map(|s| s.to_string()).collect(),
        };
        let p: Permissions = [("a".to_string(), ut(&["search", "fetch"], &["fetch"]))].into();
        assert_eq!(permitted(&p, "a"), ["search"]);
        assert!(permitted(&p, "nobody").is_empty() && !holds(&p, "nobody", "search"));
        assert!(valid(&p));
        assert!(!valid(&[("a".to_string(), ut(&["shell"], &[]))].into()));
        assert!(!valid(&[(String::new(), ut(&[], &[]))].into()));
    }
}
