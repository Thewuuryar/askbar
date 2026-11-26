//! The PII settings the admin pushes to the gateway (plan PII-P1.1): whether detection runs, what it does, and the
//! terms to always or never redact. Left out of a configuration, detection is off.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `detect_only` counts what it finds and changes nothing; `enforce` acts on what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiMode {
    #[default]
    DetectOnly,
    Enforce,
}

/// What to do with a value of one type: replace it with a placeholder and restore it in the reply, refuse the turn,
/// or let it through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiAction {
    Pseudonymize,
    Block,
    Allow,
}

/// An email address.
pub const EMAIL: &str = "EMAIL";
/// A telephone number.
pub const PHONE: &str = "PHONE";
/// A payment card number, checked with the Luhn check.
pub const CARD: &str = "CARD";
/// A bank account number, IBAN, checked with its check digits.
pub const IBAN: &str = "IBAN";
/// A national identity number.
pub const NATIONAL_ID: &str = "NATIONAL_ID";
/// An IP address.
pub const IP: &str = "IP";
/// A provider API key.
pub const API_KEY: &str = "API_KEY";
/// A GitHub token.
pub const GITHUB_TOKEN: &str = "GITHUB_TOKEN";
/// A JSON Web Token.
pub const JWT: &str = "JWT";
/// A private key, PEM-encoded.
pub const PRIVATE_KEY: &str = "PRIVATE_KEY";
/// A term the admin listed in `deny_terms`.
pub const CUSTOM: &str = "CUSTOM";

/// Every entity type name a configuration may name in `entities`.
pub const PII_TYPES: [&str; 11] = [EMAIL, PHONE, CARD, IBAN, NATIONAL_ID, IP, API_KEY, GITHUB_TOKEN, JWT, PRIVATE_KEY, CUSTOM];

/// Most terms one list (`allow_terms`, `deny_terms`) may hold.
pub const MAX_TERMS: usize = 500;
/// Fewest characters a term may have.
pub const MIN_TERM_CHARS: usize = 1;
/// Most characters a term may have.
pub const MAX_TERM_CHARS: usize = 100;

/// The PII settings. The default is off: no detection, `detect_only`, the default action for each type, no terms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PiiConfig {
    pub enabled: bool,
    pub mode: PiiMode,
    /// Type to action (`CUSTOM` is not configurable). A type left out takes its default action (see `action_for`).
    pub entities: BTreeMap<String, PiiAction>,
    /// Values never to redact (for example a known internal address).
    pub allow_terms: Vec<String>,
    /// Values always to pseudonymize, as `CUSTOM`.
    pub deny_terms: Vec<String>,
}

impl Default for PiiConfig {
    fn default() -> Self {
        Self { enabled: false, mode: PiiMode::DetectOnly, entities: BTreeMap::new(), allow_terms: vec![], deny_terms: vec![] }
    }
}

impl PiiConfig {
    /// Left out of the JSON while it is the default, so a configuration without PII settings keeps its version.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The action for a type: the configured one, else the default from the plan (secrets, private keys and card
    /// numbers block; email, phone, IBAN, national ID and custom terms pseudonymize; the rest allow).
    pub fn action_for(&self, ty: &str) -> PiiAction {
        if ty == CUSTOM {
            return PiiAction::Pseudonymize; // not configurable: the admin's own terms are always replaced
        }
        self.entities.get(ty).copied().unwrap_or(match ty {
            API_KEY | GITHUB_TOKEN | JWT | PRIVATE_KEY | CARD => PiiAction::Block,
            EMAIL | PHONE | IBAN | NATIONAL_ID | CUSTOM => PiiAction::Pseudonymize,
            _ => PiiAction::Allow,
        })
    }

    /// Refuses what the receiver must not take: an unknown type, too many entries, or a term that is empty, blank,
    /// too long or holds a control character.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.entities.len() > PII_TYPES.len() || self.entities.keys().any(|k| k == CUSTOM || !PII_TYPES.contains(&k.as_str())) {
            return Err("entities names an unknown or fixed type");
        }
        for list in [&self.allow_terms, &self.deny_terms] {
            if list.len() > MAX_TERMS {
                return Err("a term list has too many terms");
            }
            for t in list {
                let n = t.chars().count();
                if t.trim().is_empty() || !(MIN_TERM_CHARS..=MAX_TERM_CHARS).contains(&n) || t.chars().any(char::is_control) {
                    return Err("a term is empty, too long or holds a control character");
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_off_by_default_and_each_type_has_its_default_action() {
        let d = PiiConfig::default();
        assert_eq!((d.enabled, d.mode), (false, PiiMode::DetectOnly));
        assert!(d.is_default() && d.validate().is_ok());
        for ty in [API_KEY, GITHUB_TOKEN, JWT, PRIVATE_KEY, CARD] {
            assert_eq!(d.action_for(ty), PiiAction::Block, "{ty}");
        }
        for ty in [EMAIL, PHONE, IBAN, NATIONAL_ID, CUSTOM] {
            assert_eq!(d.action_for(ty), PiiAction::Pseudonymize, "{ty}");
        }
        assert_eq!(d.action_for(IP), PiiAction::Allow);
    }

    #[test]
    fn a_configured_action_overrides_the_default() {
        let mut c = PiiConfig::default();
        c.entities.insert(EMAIL.into(), PiiAction::Allow);
        c.entities.insert(CARD.into(), PiiAction::Pseudonymize);
        c.entities.insert(CUSTOM.into(), PiiAction::Allow);
        assert_eq!(c.action_for(CUSTOM), PiiAction::Pseudonymize, "CUSTOM ignores entities");
        assert_eq!(
            (c.action_for(EMAIL), c.action_for(CARD), c.action_for(PHONE)),
            (PiiAction::Allow, PiiAction::Pseudonymize, PiiAction::Pseudonymize)
        );
        assert!(!c.is_default());
    }

    #[test]
    fn validate_refuses_an_unknown_type_an_empty_term_and_too_many_terms() {
        let ok = |c: &PiiConfig| c.validate().is_ok();
        let mut c = PiiConfig::default();
        c.entities.insert("PERSON".into(), PiiAction::Block);
        assert!(!ok(&c), "PERSON is not an entity type");
        let mut c = PiiConfig { allow_terms: vec!["  ".into()], ..Default::default() };
        assert!(!ok(&c), "a blank term");
        c.allow_terms = vec!["".into()];
        assert!(!ok(&c), "an empty term");
        c.allow_terms = vec!["a".repeat(MAX_TERM_CHARS + 1)];
        assert!(!ok(&c), "a term over the length limit");
        c.allow_terms = vec!["a\nb".into()];
        assert!(!ok(&c), "a control character");
        c.allow_terms = vec!["x".repeat(MAX_TERM_CHARS)];
        assert!(ok(&c), "a term at the limit is fine");
        c.deny_terms = vec!["t".into(); MAX_TERMS + 1];
        assert!(!ok(&c), "too many terms");
        c.deny_terms = vec!["t".into(); MAX_TERMS];
        assert!(ok(&c));
        c.entities = PII_TYPES.iter().filter(|t| **t != CUSTOM).map(|t| (t.to_string(), PiiAction::Allow)).collect();
        assert!(ok(&c), "every configurable type once is at most the list");
        c.entities.insert(CUSTOM.into(), PiiAction::Allow);
        assert!(!ok(&c), "CUSTOM is not configurable");
    }

    #[test]
    fn the_enums_are_snake_case_on_the_wire() {
        assert_eq!(serde_json::to_value(PiiMode::DetectOnly).unwrap(), "detect_only");
        assert_eq!(serde_json::from_value::<PiiMode>(serde_json::json!("enforce")).unwrap(), PiiMode::Enforce);
        assert_eq!(serde_json::to_value(PiiAction::Pseudonymize).unwrap(), "pseudonymize");
        assert_eq!(serde_json::from_value::<PiiAction>(serde_json::json!("block")).unwrap(), PiiAction::Block);
        assert!(serde_json::from_value::<PiiAction>(serde_json::json!("redact")).is_err());
        let c = PiiConfig { enabled: true, mode: PiiMode::Enforce, ..Default::default() };
        let back: PiiConfig = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
