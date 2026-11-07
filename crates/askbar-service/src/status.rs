//! Which configuration a service runs, and which process it is.

/// A new ID for every start of the process (a ULID). The admin sees a restart as a new boot ID and
/// pushes configuration again (ADR 0028 rule 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootId(String);

impl BootId {
    pub fn new() -> BootId {
        BootId(ulid::Ulid::new().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for BootId {
    fn default() -> Self {
        BootId::new()
    }
}

/// The `GET /status` body: `{"config_version": <n or null>, "boot_id": "..."}`.
pub fn body(config_version: Option<u64>, boot_id: &BootId) -> serde_json::Value {
    serde_json::json!({ "config_version": config_version, "boot_id": boot_id.as_str() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_boot_id_is_new_and_the_body_names_both_fields() {
        let (a, b) = (BootId::new(), BootId::new());
        assert_ne!(a, b);
        assert_eq!(a.as_str().len(), 26);
        assert_eq!(body(None, &a), serde_json::json!({ "config_version": null, "boot_id": a.as_str() }));
        assert_eq!(body(Some(7), &a)["config_version"], 7);
    }
}
