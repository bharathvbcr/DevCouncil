//! Artifact writers with tmp+rename (V14) and fingerprint skip-on-unchanged.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::escape::{html_escape, json_script_escape};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactFingerprint {
    pub generated_head: String,
    pub built_at: u64,
    pub fingerprint: String,
}

/// Write bytes via tmp+rename; returns true when content changed on disk.
pub fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<bool> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(content)?;
    file.sync_all()?;
    if path.exists() {
        let existing = fs::read(path)?;
        if existing == content {
            fs::remove_file(&tmp).ok();
            return Ok(false);
        }
    }
    fs::rename(&tmp, path)?;
    Ok(true)
}

/// Skip regeneration when fingerprint matches existing artifact header (V14).
pub fn should_regenerate(path: &Path, fp: &ArtifactFingerprint) -> bool {
    if !path.exists() {
        return true;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return true;
    };
    let marker = format!("fingerprint:{}", fp.fingerprint);
    let escaped_marker = format!("fingerprint:{}", html_escape(&fp.fingerprint));
    !text.contains(&marker) && !text.contains(&escaped_marker)
}

/// Minimal subsystem map HTML with esc() at every sink (V1).
pub fn render_subsystem_map_html(
    title: &str,
    subsystems: &[(&str, &[String])],
    fp: &ArtifactFingerprint,
) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<!-- fingerprint:{} -->\n<h1>{}</h1>\n<p>head={} built_at={} fp={}</p>\n",
        html_escape(&fp.fingerprint),
        html_escape(title),
        html_escape(&fp.generated_head),
        fp.built_at,
        html_escape(&fp.fingerprint)
    ));
    for (area, files) in subsystems {
        body.push_str(&format!("<h2>{}</h2>\n<ul>\n", html_escape(area)));
        for f in *files {
            body.push_str(&format!("<li>{}</li>\n", html_escape(f)));
        }
        body.push_str("</ul>\n");
    }
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body>{}</body></html>",
        html_escape(title),
        body
    )
}

/// Symbol explorer payload embedded in script tag (V2) with escaped title (V1).
pub fn render_symbol_explorer_html(
    title: &str,
    payload_json: &str,
    fp: &ArtifactFingerprint,
) -> String {
    let safe_title = html_escape(title);
    let safe_json = json_script_escape(payload_json);
    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{safe_title}</title></head><body>\
         <!-- fingerprint:{} -->\
         <h1>{safe_title}</h1>\
         <p>staleness: head={} fp={}</p>\
         <script type=\"application/json\" id=\"payload\">{safe_json}</script>\
         </body></html>",
        html_escape(&fp.fingerprint),
        html_escape(&fp.generated_head),
        html_escape(&fp.fingerprint)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fp() -> ArtifactFingerprint {
        ArtifactFingerprint {
            generated_head: "abc123".into(),
            built_at: 1,
            fingerprint: "fp-test".into(),
        }
    }

    #[test]
    fn test_v14_atomic_write_and_fingerprint_skip() {
        // closes V14
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("devmap-artifact-{stamp}.html"));
        let html = render_subsystem_map_html("Test", &[("core", &["a.py".to_string()])], &fp());
        assert!(write_atomic(&path, html.as_bytes()).unwrap());
        assert!(!should_regenerate(&path, &fp()));
        let fp2 = ArtifactFingerprint {
            fingerprint: "other".into(),
            ..fp()
        };
        assert!(should_regenerate(&path, &fp2));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_v1_hostile_name_in_subsystem_html() {
        // closes V1
        let hostile = "x<img src=x onerror=alert(1)>.ts";
        let html = render_subsystem_map_html(hostile, &[("area", &[hostile.to_string()])], &fp());
        assert!(!html.contains("<img"));
        assert!(html.contains("x&lt;img"));
    }

    #[test]
    fn test_v1_v2_symbol_payload_is_inert_and_remains_valid_json() {
        let payload = serde_json::json!({
            "name": "</script><img src=x onerror=alert(1)>",
            "ampersand": "a&b"
        });
        let raw = serde_json::to_string(&payload).unwrap();
        let html = render_symbol_explorer_html("Symbols", &raw, &fp());
        let marker = "<script type=\"application/json\" id=\"payload\">";
        let start = html.find(marker).unwrap() + marker.len();
        let end = html[start..].find("</script>").unwrap() + start;
        let embedded = &html[start..end];

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(embedded).unwrap(),
            payload
        );
        assert_eq!(html.matches("</script>").count(), 1);
        assert!(!html.contains("<img"));
    }
}
