//! Curve naming reuses Core's NFKC, whitespace and full Unicode case fold.

use super::{presets::builtin_presets, EffectsError, PresetId, UserPreset, MAX_USER_PRESETS};
use echo_core::domain::text::{grapheme_count, normalize_text, normalized_key};

/// Normalize display text and reject collisions in both namespaces.
/// # Errors
/// Returns `InvalidName` or `DuplicateName`; `excluding` permits own-name rename.
pub fn validate_name(
    name: &str,
    users: &[UserPreset],
    excluding: Option<&PresetId>,
) -> Result<String, EffectsError> {
    let display = normalize_text(name);
    if !(1..=40).contains(&grapheme_count(&display)) {
        return Err(EffectsError::InvalidName);
    }
    let key = normalized_key(&display);
    if builtin_presets()
        .iter()
        .any(|preset| normalized_key(&preset.name) == key)
        || users
            .iter()
            .any(|preset| Some(&preset.id) != excluding && normalized_key(&preset.name) == key)
    {
        return Err(EffectsError::DuplicateName);
    }
    Ok(display)
}

/// # Errors
/// Rejects new entries once the user registry reaches 50; drafts do not count.
pub const fn validate_capacity(users: &[UserPreset]) -> Result<(), EffectsError> {
    if users.len() >= MAX_USER_PRESETS {
        Err(EffectsError::QuotaExceeded)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EqCurve;

    #[test]
    fn normalized_grapheme_boundaries_and_full_casefold() {
        assert_eq!(validate_name("　 e\u{301} \t", &[], None).unwrap(), "é");
        assert!(validate_name(&"👨‍👩‍👧‍👦".repeat(40), &[], None).is_ok());
        assert_eq!(
            validate_name(&"👨‍👩‍👧‍👦".repeat(41), &[], None),
            Err(EffectsError::InvalidName)
        );
        assert_eq!(
            validate_name(" \t　", &[], None),
            Err(EffectsError::InvalidName)
        );
        assert_eq!(
            validate_name("古典", &[], None),
            Err(EffectsError::DuplicateName)
        );
        let user = UserPreset {
            id: PresetId::new_user(),
            name: "Straße".into(),
            curve: EqCurve::default(),
        };
        assert_eq!(
            validate_name("ＳＴＲＡＳＳＥ", &[user], None),
            Err(EffectsError::DuplicateName)
        );
    }

    #[test]
    fn quota_and_rename_are_independent() {
        let mut users: Vec<_> = (0..49)
            .map(|index| UserPreset {
                id: PresetId::new_user(),
                name: format!("curve {index}"),
                curve: EqCurve::default(),
            })
            .collect();
        assert!(validate_capacity(&users).is_ok());
        users.push(UserPreset {
            id: PresetId::new_user(),
            name: "last".into(),
            curve: EqCurve::default(),
        });
        assert_eq!(validate_capacity(&users), Err(EffectsError::QuotaExceeded));
        assert!(validate_name("last", &users, Some(&users[49].id)).is_ok());
    }
}
