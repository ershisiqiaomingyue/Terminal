//! One-off per-turn strategy switches.

/// Spend the recovery reasoning effort on at most one request, then return to
/// the user's normal selection. A line without a recovery target keeps the
/// normal value instead of dropping it.
pub(super) fn take_recovery_reasoning_effort(
    armed: &mut bool,
    recovery: Option<&String>,
    normal: &Option<String>,
) -> Option<String> {
    if *armed {
        *armed = false;
        if let Some(effort) = recovery {
            return Some(effort.clone());
        }
    }
    normal.clone()
}

#[cfg(test)]
mod tests {
    use super::take_recovery_reasoning_effort;

    #[test]
    fn the_recovery_effort_is_spent_on_one_round_only() {
        let recovery = Some("minimal".to_string());
        let normal = Some("medium".to_string());
        let mut armed = true;
        assert_eq!(
            take_recovery_reasoning_effort(&mut armed, recovery.as_ref(), &normal).as_deref(),
            Some("minimal")
        );
        assert!(!armed);
        assert_eq!(
            take_recovery_reasoning_effort(&mut armed, recovery.as_ref(), &normal).as_deref(),
            Some("medium")
        );
    }

    #[test]
    fn an_unarmed_switch_never_touches_the_normal_effort() {
        let recovery = Some("minimal".to_string());
        let normal = None::<String>;
        let mut armed = false;
        assert_eq!(
            take_recovery_reasoning_effort(&mut armed, recovery.as_ref(), &normal),
            None
        );
        assert!(!armed);
    }

    #[test]
    fn arming_without_a_recovery_target_still_falls_back_to_the_normal_effort() {
        let normal = Some("high".to_string());
        let mut armed = true;
        assert_eq!(
            take_recovery_reasoning_effort(&mut armed, None, &normal).as_deref(),
            Some("high")
        );
        assert!(!armed);
    }
}
