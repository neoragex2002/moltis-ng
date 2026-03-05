use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub enum PromptSubstError {
    MissingVar { var: String },
    UnreplacedVar { var: String },
}

impl std::fmt::Display for PromptSubstError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingVar { var } => write!(
                f,
                "PROMPT_TEMPLATE_MISSING_VAR: template references `{{{{{var}}}}}` but no value was provided"
            ),
            Self::UnreplacedVar { var } => write!(
                f,
                "PROMPT_TEMPLATE_UNREPLACED_VAR: rendered output still contains `{{{{{var}}}}}`"
            ),
        }
    }
}

impl std::error::Error for PromptSubstError {}

const ESC_LBRACE: &str = "<<MOLTIS_LBRACE>>";
const ESC_RBRACE: &str = "<<MOLTIS_RBRACE>>";

fn is_valid_var_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn unescape_braces(mut s: String) -> String {
    if s.contains(ESC_LBRACE) {
        s = s.replace(ESC_LBRACE, "{{");
    }
    if s.contains(ESC_RBRACE) {
        s = s.replace(ESC_RBRACE, "}}");
    }
    s
}

fn escape_braces_for_validation(mut s: String) -> String {
    // Allow literal `{{var}}` in final output via `{{{{ ... }}}}` in the template.
    // We replace escape sequences with sentinels during parsing and only restore
    // braces after validation completes.
    if s.contains("{{{{") {
        s = s.replace("{{{{", ESC_LBRACE);
    }
    if s.contains("}}}}") {
        s = s.replace("}}}}", ESC_RBRACE);
    }
    s
}

/// Extract strict `{{var}}` placeholders from a template.
///
/// Strict placeholders must be written with no spaces, and `var` must match
/// `[a-z0-9_]+`. Non-matching `{{ ... }}` sequences are treated as literal text.
pub fn extract_strict_vars(template: &str) -> BTreeSet<String> {
    let s = escape_braces_for_validation(template.to_string());
    let bytes = s.as_bytes();
    let mut out = BTreeSet::new();
    let mut i: usize = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find closing braces.
            let mut j = i + 2;
            while j + 1 < bytes.len() {
                if bytes[j] == b'}' && bytes[j + 1] == b'}' {
                    break;
                }
                j += 1;
            }
            if j + 1 >= bytes.len() {
                break;
            }
            let name = &s[i + 2..j];
            if is_valid_var_name(name) {
                out.insert(name.to_string());
            }
            i = j + 2;
            continue;
        }
        i += 1;
    }

    out
}

/// Render a template by replacing strict `{{var}}` placeholders with values.
///
/// Notes:
/// - Only strict placeholders (`{{var}}`, `var` matches `[a-z0-9_]+`) are replaced.
/// - Non-matching `{{ ... }}` sequences are treated as literal text.
/// - Escapes are supported: `{{{{` → literal `{{`, `}}}}` → literal `}}`.
/// - Replacement is single-pass and non-recursive.
pub fn render_strict_template(
    template: &str,
    vars: &BTreeMap<String, String>,
) -> Result<String, PromptSubstError> {
    let s = escape_braces_for_validation(template.to_string());
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(s.len() + 128);
    let mut i: usize = 0;

    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find closing braces.
            let mut j = i + 2;
            while j + 1 < bytes.len() {
                if bytes[j] == b'}' && bytes[j + 1] == b'}' {
                    break;
                }
                j += 1;
            }
            if j + 1 >= bytes.len() {
                // Malformed; emit literal.
                out.extend_from_slice(b"{{");
                i += 2;
                continue;
            }
            let name = &s[i + 2..j];
            if is_valid_var_name(name) {
                let Some(val) = vars.get(name) else {
                    return Err(PromptSubstError::MissingVar {
                        var: name.to_string(),
                    });
                };
                out.extend_from_slice(val.as_bytes());
                i = j + 2;
                continue;
            }

            // Not a strict var; treat as literal.
            out.extend_from_slice(b"{{");
            i += 2;
            continue;
        }

        out.push(bytes[i]);
        i += 1;
    }

    let result = String::from_utf8(out).expect("render_strict_template: internal utf8 invariant");

    // Validate there are no remaining strict placeholders.
    for var in extract_strict_vars(&result) {
        return Err(PromptSubstError::UnreplacedVar { var });
    }

    Ok(unescape_braces(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_strict_vars() {
        let vars = extract_strict_vars("a {{foo}} b {{ bar }} c {{baz_1}}");
        let got: Vec<_> = vars.into_iter().collect();
        assert_eq!(got, vec!["baz_1".to_string(), "foo".to_string()]);
    }

    #[test]
    fn renders_strict_vars() {
        let mut vars = BTreeMap::new();
        vars.insert("foo".to_string(), "X".to_string());
        vars.insert("bar_1".to_string(), "Y".to_string());
        let out = render_strict_template("{{foo}} {{ bar }} {{bar_1}}", &vars).unwrap();
        assert_eq!(out, "X {{ bar }} Y");
    }

    #[test]
    fn missing_var_fails_fast() {
        let vars = BTreeMap::new();
        let err = render_strict_template("{{foo}}", &vars).unwrap_err();
        assert!(err.to_string().contains("PROMPT_TEMPLATE_MISSING_VAR"));
    }

    #[test]
    fn escape_allows_literal_placeholder_text() {
        let vars = BTreeMap::new();
        let out = render_strict_template("{{{{foo}}}}", &vars).unwrap();
        assert_eq!(out, "{{foo}}");
    }

    #[test]
    fn preserves_utf8_text_outside_placeholders() {
        let mut vars = BTreeMap::new();
        vars.insert("who".to_string(), "世界".to_string());
        let out = render_strict_template("你好，{{who}}！", &vars).unwrap();
        assert_eq!(out, "你好，世界！");
    }
}
