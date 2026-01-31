//! Template loading, selection, and fallback.

use crate::config::TemplateConfig;
use crate::storage::file::get_roz_home;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Default block template used when custom templates are not available.
pub const DEFAULT_BLOCK_TEMPLATE: &str = r#"Review required before exit.

Use the **Task** tool with these parameters:

- `subagent_type`: `"roz:roz"`
- `model`: `"opus"`

Prompt template:

```
SESSION_ID={{session_id}}

## Summary
[What you did and why]

## Files Changed
[List of modified files]
```
"#;

/// Load a template by ID.
///
/// First checks `~/.roz/templates/block-{id}.md`, falls back to default.
#[must_use]
pub fn load_template(id: &str) -> String {
    load_template_from(id, &get_roz_home())
}

/// Load a template by ID from a specific base directory.
///
/// This is primarily for testing - allows specifying a custom base directory.
#[must_use]
pub fn load_template_from(id: &str, base_dir: &Path) -> String {
    let path = base_dir.join("templates").join(format!("block-{id}.md"));

    match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => {
            // Fall back to default template
            DEFAULT_BLOCK_TEMPLATE.to_string()
        }
    }
}

/// Select a template ID based on configuration.
///
/// If `active` is "random", uses weighted random selection from the weights map.
/// Otherwise, returns the `active` template ID directly.
#[must_use]
pub fn select_template(config: &TemplateConfig) -> String {
    match config.active.as_str() {
        "random" => weighted_random(&config.weights),
        specific => specific.to_string(),
    }
}

/// Select a random template ID based on weights.
///
/// Weights determine probability: `{"v1": 70, "v2": 30}` means 70% chance of v1.
/// Returns "default" if weights are empty.
#[must_use]
#[allow(clippy::implicit_hasher)] // Internal function, no need to generalize over hasher
pub fn weighted_random(weights: &HashMap<String, u32>) -> String {
    if weights.is_empty() {
        return "default".to_string();
    }

    let total: u32 = weights.values().sum();
    if total == 0 {
        return weights
            .keys()
            .next()
            .cloned()
            .unwrap_or_else(|| "default".to_string());
    }

    // Use a simple deterministic random based on current time nanoseconds
    // This avoids adding a rand dependency while still providing reasonable distribution
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // Safe: modulo by total (u32) guarantees result fits in u32
    #[allow(clippy::cast_possible_truncation)]
    let roll = (now % u128::from(total)) as u32;

    let mut cumulative = 0;
    for (template_id, weight) in weights {
        cumulative += weight;
        if roll < cumulative {
            return template_id.clone();
        }
    }

    // Fallback (shouldn't reach here)
    weights
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| "default".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_template_fallback() {
        let temp_dir = TempDir::new().unwrap();
        // With no template file, should return default
        let template = load_template_from("nonexistent", temp_dir.path());
        assert!(template.contains("SESSION_ID={{session_id}}"));
        assert!(template.contains("roz:roz"));
    }

    #[test]
    fn load_template_custom() {
        let temp_dir = TempDir::new().unwrap();

        // Create custom template
        let templates_dir = temp_dir.path().join("templates");
        fs::create_dir_all(&templates_dir).unwrap();
        fs::write(
            templates_dir.join("block-custom.md"),
            "Custom template for {{session_id}}",
        )
        .unwrap();

        let template = load_template_from("custom", temp_dir.path());
        assert_eq!(template, "Custom template for {{session_id}}");
    }

    #[test]
    fn default_template_has_session_id_placeholder() {
        assert!(DEFAULT_BLOCK_TEMPLATE.contains("{{session_id}}"));
    }

    #[test]
    fn default_template_mentions_roz_agent() {
        assert!(DEFAULT_BLOCK_TEMPLATE.contains("roz:roz"));
    }

    #[test]
    fn template_substitution() {
        let template = DEFAULT_BLOCK_TEMPLATE;
        let result = template.replace("{{session_id}}", "test-123");
        assert!(result.contains("SESSION_ID=test-123"));
        assert!(!result.contains("{{session_id}}"));
    }

    #[test]
    fn select_template_specific() {
        let config = TemplateConfig {
            active: "v2".to_string(),
            weights: HashMap::new(),
        };
        assert_eq!(select_template(&config), "v2");
    }

    #[test]
    fn select_template_default() {
        let config = TemplateConfig::default();
        assert_eq!(select_template(&config), "default");
    }

    #[test]
    fn weighted_random_empty_weights() {
        let weights: HashMap<String, u32> = HashMap::new();
        assert_eq!(weighted_random(&weights), "default");
    }

    #[test]
    fn weighted_random_single_option() {
        let mut weights = HashMap::new();
        weights.insert("v1".to_string(), 100);
        // With only one option, it should always return that option
        assert_eq!(weighted_random(&weights), "v1");
    }

    #[test]
    fn weighted_random_all_zero_weights() {
        let mut weights = HashMap::new();
        weights.insert("v1".to_string(), 0);
        weights.insert("v2".to_string(), 0);
        // With all zero weights, should return one of the keys
        let result = weighted_random(&weights);
        assert!(result == "v1" || result == "v2");
    }

    #[test]
    fn weighted_random_returns_valid_template() {
        let mut weights = HashMap::new();
        weights.insert("v1".to_string(), 50);
        weights.insert("v2".to_string(), 50);

        // Run multiple times to verify distribution (not a perfect test, but sanity check)
        let mut found_templates = std::collections::HashSet::new();
        for _ in 0..100 {
            let result = weighted_random(&weights);
            assert!(
                result == "v1" || result == "v2",
                "Unexpected template: {result}"
            );
            found_templates.insert(result);
        }
        // With 50/50 weights, we should see both templates across 100 iterations
        // (statistically very unlikely to not see both)
        assert!(
            found_templates.len() == 2,
            "Expected both templates to be selected at least once"
        );
    }

    #[test]
    fn select_template_random_mode() {
        let mut weights = HashMap::new();
        weights.insert("v1".to_string(), 100);
        let config = TemplateConfig {
            active: "random".to_string(),
            weights,
        };
        // With 100% weight on v1, random should always return v1
        assert_eq!(select_template(&config), "v1");
    }
}
