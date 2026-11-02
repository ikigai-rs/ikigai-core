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
/// not contradict the other.
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
        && b_params.iter().all(|(name, value)| {
            a_params
                .iter()
                .find(|(n, _)| n == name)
                .is_none_or(|(_, v)| v == value)
        })
}

/// A media type's essence and its parameters (names lowercased, values unquoted, `charset`
/// values lowercased), for [`media_types_agree`] only.
fn media_type_parts(media_type: &str) -> (String, Vec<(String, String)>) {
    let params = media_type
        .split(';')
        .skip(1)
        .filter_map(|param| {
            let (name, value) = param.split_once('=')?;
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            let value = if name == "charset" {
                value.to_ascii_lowercase()
            } else {
                value.to_string()
            };
            Some((name, value))
        })
        .collect();
    (media_type_essence(media_type), params)
}
