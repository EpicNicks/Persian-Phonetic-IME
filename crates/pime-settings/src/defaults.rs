//! The commented default `config.toml` (`--print-default`, `--reset`, and the
//! starting point when the settings app creates the file).
//!
//! The text is written by hand from `docs/config-schema.md` and the `qamooscheh`
//! preset; the tests below keep it honest against `pime-core` (it loads with no
//! problems, resolves exactly to the built-in default, and names every setting in
//! `option_specs()` / `autocomplete_specs()` with every allowed value and its default).

/// The default file, every option present with its default value and explained.
pub const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[cfg(test)]
mod tests {
    use super::*;
    use pime_core::{
        autocomplete_specs, builtin_preset_names, load_config, option_specs, ResolvedConfig,
    };

    #[test]
    fn loads_cleanly_and_equals_builtin_default() {
        let loaded = load_config(DEFAULT_CONFIG);
        assert!(
            loaded.issues.is_empty(),
            "{}",
            pime_core::format_issues(&loaded.issues)
        );
        assert_eq!(loaded.config, ResolvedConfig::default());
    }

    #[test]
    fn mentions_every_option_and_value() {
        // Every setting appears as "name = ..." (or "# name = ..." for the list options,
        // which are commented out so the preset's lists apply), with every allowed value.
        for spec in option_specs().iter().chain(autocomplete_specs()) {
            let name = spec.name;
            assert!(
                DEFAULT_CONFIG.contains(&format!("\n{name} = "))
                    || DEFAULT_CONFIG.contains(&format!("\n# {name} = ")),
                "{name} missing"
            );
            for v in spec.allowed_values() {
                assert!(DEFAULT_CONFIG.contains(&format!("\"{v}\"")), "{name}: {v}");
            }
        }
        for name in builtin_preset_names() {
            assert!(
                DEFAULT_CONFIG.contains(&format!("\"{name}\"")),
                "preset {name}"
            );
        }
    }

    #[test]
    fn active_values_are_the_core_defaults() {
        let doc: toml_edit::DocumentMut = DEFAULT_CONFIG.parse().unwrap();
        for (table, specs) in [
            ("options", option_specs()),
            ("autocomplete", autocomplete_specs()),
        ] {
            for spec in specs {
                let Some(written) = doc[table].get(spec.name).and_then(|i| i.as_value()) else {
                    continue; // commented-out list options
                };
                let expected = spec.default_value().map(|v| crate::edit::to_toml_value(&v));
                assert_eq!(
                    Some(written.to_string().trim().to_string()),
                    expected.map(|v| v.to_string().trim().to_string()),
                    "{table}.{}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn commented_examples_are_valid_when_uncommented() {
        // Uncomment every "# key = [..." example line (and allow sequences for the one
        // that needs it): the result must still load without errors (the tch example
        // gives the documented "unreachable prefix" warning).
        let mut text = String::new();
        for line in DEFAULT_CONFIG.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("# ") {
                if rest.contains(" = [") {
                    text.push_str(rest.split("  #").next().unwrap());
                    text.push('\n');
                    continue;
                }
            }
            text.push_str(&line.replace("allow_sequences = false", "allow_sequences = true"));
            text.push('\n');
        }
        let loaded = load_config(&text);
        let report = pime_core::format_issues(&loaded.issues);
        assert_eq!(
            pime_core::fallback_of(&loaded.issues),
            pime_core::Fallback::None,
            "{report}"
        );
        assert_eq!(
            loaded.config.candidates("tch"),
            Some(&["چ".to_string()][..])
        );
        assert_eq!(loaded.config.candidates("x").unwrap().len(), 2);
    }

    #[test]
    fn uses_only_persian_forms_outside_comments() {
        for line in DEFAULT_CONFIG
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
        {
            assert!(
                !line.contains(['\u{064A}', '\u{0643}', '\u{0629}']),
                "{line}"
            );
        }
    }
}
