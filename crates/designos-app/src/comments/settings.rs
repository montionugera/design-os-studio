//! The "Your name" app setting, stored in `settings.json` in the library root.
//!
//! The app had no settings store, so this is a small JSON object; keys it does not
//! know are preserved so other settings can live beside `author`.

use std::path::Path;

pub const FILE: &str = "settings.json";

/// The OS user name, or "You" when it cannot be determined.
pub fn os_user_name() -> String {
    ["USERNAME", "USER", "LOGNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "You".into())
}

fn read(root: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(root.join(FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The saved author name, if one was set.
pub fn load_author(root: &Path) -> Option<String> {
    read(root).get("author").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A boolean app setting stored beside the author name.
pub fn load_flag(root: &Path, key: &str) -> Option<bool> {
    read(root).get(key).and_then(|v| v.as_bool())
}

pub fn save_flag(root: &Path, key: &str, value: bool) -> Result<(), String> {
    let mut map = read(root);
    map.insert(key.into(), value.into());
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    designos_library::write_atomic(&root.join(FILE), text.as_bytes()).map_err(|e| format!("Cannot save settings: {e}"))
}

/// Saves the author name (an empty name resets to the OS user name).
pub fn save_author(root: &Path, name: &str) -> Result<(), String> {
    let mut map = read(root);
    let name = name.trim();
    if name.is_empty() {
        map.remove("author");
    } else {
        map.insert("author".into(), name.into());
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    designos_library::write_atomic(&root.join(FILE), text.as_bytes()).map_err(|e| format!("Cannot save settings: {e}"))
}

/// Loads user-level `auto_fix` settings from `settings.json` (`E15-13` §3.4).
#[allow(dead_code)]
pub fn load_auto_fix(root: &Path) -> Option<designos_doc::AutoFixSettings> {
    read(root).get("auto_fix").and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Saves user-level `auto_fix` settings to `settings.json` (`E15-13` §3.4).
#[allow(dead_code)]
pub fn save_auto_fix(root: &Path, settings: Option<&designos_doc::AutoFixSettings>) -> Result<(), String> {
    let mut map = read(root);
    match settings {
        Some(s) if !s.is_empty() => {
            let val = serde_json::to_value(s).map_err(|e| e.to_string())?;
            map.insert("auto_fix".into(), val);
        }
        _ => {
            map.remove("auto_fix");
        }
    }
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(map)).map_err(|e| e.to_string())?;
    designos_library::write_atomic(&root.join(FILE), text.as_bytes()).map_err(|e| format!("Cannot save settings: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("designos_settings_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn author_roundtrip_keeps_other_keys() {
        let d = tmp("a");
        assert_eq!(load_author(&d), None);
        std::fs::write(d.join(FILE), r#"{"theme":"dark","author":"old"}"#).unwrap();
        assert_eq!(load_author(&d).as_deref(), Some("old"));
        save_author(&d, "  Kir ").unwrap();
        assert_eq!(load_author(&d).as_deref(), Some("Kir"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join(FILE)).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        save_author(&d, "").unwrap();
        assert_eq!(load_author(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn broken_settings_file_is_ignored() {
        let d = tmp("b");
        std::fs::write(d.join(FILE), "{ nope").unwrap();
        assert_eq!(load_author(&d), None);
        save_author(&d, "Ann").unwrap();
        assert_eq!(load_author(&d).as_deref(), Some("Ann"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn auto_fix_roundtrip_keeps_other_keys() {
        let d = tmp("af");
        assert_eq!(load_auto_fix(&d), None);
        let mut af = designos_doc::AutoFixSettings { mode: Some(designos_doc::AutoFixMode::ReportOnly), ..Default::default() };
        af.rules.insert("sizing-conflict".into(), false);
        save_auto_fix(&d, Some(&af)).unwrap();
        assert_eq!(load_auto_fix(&d), Some(af));
        save_auto_fix(&d, None).unwrap();
        assert_eq!(load_auto_fix(&d), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn os_user_is_never_empty() {
        assert!(!os_user_name().is_empty());
    }
}
