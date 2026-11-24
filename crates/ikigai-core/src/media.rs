//! Media types compared as media types, not strings (RFC 9110 §8.3.1).
//!
//! One implementation for the whole ecosystem (ledger #2877): selection uses it to match a
//! `want=` against declared outputs and a transreption against declared conversions, and a
//! module asking "is this JSON?" or "is this the type I serve?" calls it rather than
//! splitting on `;` by hand. No validation anywhere: a malformed string compares as whatever
//! it splits into, never as an error.

/// A media type's essence: the `type/subtype` before any parameter, whitespace trimmed and
/// ASCII-lowercased (RFC 9110 §8.3.1: type and subtype are case-insensitive).
///
/// ```
/// use ikigai_core::media_type_essence;
///
/// assert_eq!(media_type_essence("Application/JSON; charset=UTF-8"), "application/json");
/// assert_eq!(media_type_essence("text/turtle"), "text/turtle");
/// // A structured-syntax suffix is part of the essence, so a caller tests it directly.
/// assert!(media_type_essence("application/ld+json;profile=\"x\"").ends_with("+json"));
/// ```
pub fn media_type_essence(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Whether two media types are one type: the same [essence](media_type_essence), and no
/// parameter that BOTH state with different values. Parameter names compare
/// case-insensitively and values after unquoting; a `charset` value case-insensitively (RFC
/// 9110 §8.3.2), every other value exactly. Symmetric: a parameter only one side states does
/// not contradict the other. When both sides state a parameter, every occurrence must agree.
///
/// This is the comparison behind `want=` ([`ActionQuery::want`](crate::ActionQuery::want))
/// and transreptor selection ([`select_transreptor`](crate::select_transreptor)).
///
/// ```
/// use ikigai_core::media_types_agree;
///
/// assert!(media_types_agree("text/csv", "Text/CSV; charset=\"UTF-8\""));
/// assert!(!media_types_agree("text/csv; charset=utf-8", "text/csv; charset=iso-8859-1"));
/// assert!(!media_types_agree("text/csv", "text/tab-separated-values"));
/// ```
pub fn media_types_agree(a: &str, b: &str) -> bool {
    let (a_essence, a_params) = media_type_parts(a);
    let (b_essence, b_params) = media_type_parts(b);
    a_essence == b_essence
        && b_params
            .iter()
            .all(|(name, value)| a_params.iter().all(|(n, v)| n != name || v == value))
}

/// A media type's essence and its parameters (names lowercased, values unquoted, `charset`
/// values lowercased), for [`media_types_agree`] only.
fn media_type_parts(media_type: &str) -> (String, Vec<(String, String)>) {
    let (essence, params) = media_type.split_once(';').unwrap_or((media_type, ""));
    let mut quoted = false;
    let mut escaped = false;
    let params = params
        .split(|c| {
            if escaped {
                escaped = false;
            } else if quoted && c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = !quoted;
            }
            c == ';' && !quoted
        })
        .filter_map(|param| {
            let (name, value) = param.split_once('=')?;
            let name = name.trim().to_ascii_lowercase();
            let mut value = unquote(value.trim());
            if name == "charset" {
                value.make_ascii_lowercase();
            }
            Some((name, value))
        })
        .collect();
    (media_type_essence(essence), params)
}

fn unquote(value: &str) -> String {
    let Some(quoted) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_owned();
    };
    let mut decoded = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let Some(escaped) = chars.next() else {
                return value.to_owned();
            };
            decoded.push(escaped);
        } else {
            decoded.push(c);
        }
    }
    decoded
}

pub(crate) fn push_parameter_value(out: &mut String, value: &str) {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    {
        out.push_str(value);
        return;
    }
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_parameters_compare_by_their_complete_unescaped_value() {
        assert!(media_types_agree(
            r#"Text/Example; CHARSET="UTF-8"; label="Red; Blue"; marker="A\"B\\C""#,
            r#"text/example;charset=utf-8;LABEL="Red; Blue";marker="A\"B\\\C""#,
        ));
        assert!(!media_types_agree(
            r#"text/example;label="Red; Blue""#,
            r#"text/example;label="Red; Green""#,
        ));
        assert!(!media_types_agree(
            r#"text/example;label="Red; Blue""#,
            r#"text/example;label="red; Blue""#,
        ));
    }

    #[test]
    fn repeated_parameter_comparison_is_symmetric() {
        let repeated = "text/example;level=one;LEVEL=two";
        let single = "text/example;level=one";
        assert_eq!(
            media_types_agree(repeated, single),
            media_types_agree(single, repeated),
        );
        assert!(!media_types_agree(repeated, single));
        assert!(media_types_agree(
            "text/example;level=one;LEVEL=one",
            single
        ));
    }
}
