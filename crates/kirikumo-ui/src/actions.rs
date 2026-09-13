//! The two gestures a write takes.
//!
//! Every write goes: press the action, then press a second button that
//! *names the object* (`AGENTS.md` rule 9, roadmap K6). The second button is
//! the whole safeguard — a confirmation that says *Delete* can be pressed
//! without reading, and one that says *Delete api-7d9f8c-2xk4t* cannot — so
//! its wording is decided here, where a test can insist on the name.
//!
//! The reader does not type the name. That was considered and rejected: it
//! is `kubectl delete`'s weight, and it turns the one action a person makes
//! under pressure into a spelling test. The name on the button, in mono, is
//! the same information without the friction.

use kirikumo_kube::Action;

/// Where a write is between the first gesture and the second.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Pending {
    /// Nothing armed.
    #[default]
    Idle,
    /// The first gesture was made; the next press does it.
    Armed {
        /// Which write.
        action: Action,
        /// For a scale, what was typed so far.
        replicas: String,
    },
}

impl Pending {
    /// Arm an action. A scale starts from what the object has now, so the
    /// field is never empty and *Scale api to 2* is the first thing offered.
    pub fn arm(action: Action, current_replicas: i64) -> Self {
        Self::Armed {
            action,
            replicas: match action {
                Action::Scale => current_replicas.max(0).to_string(),
                _ => String::new(),
            },
        }
    }

    /// Which action is armed, if one is.
    pub fn action(&self) -> Option<Action> {
        match self {
            Self::Armed { action, .. } => Some(*action),
            Self::Idle => None,
        }
    }

    /// The replica count typed so far, if it is a number a controller can
    /// have. `None` is a field that cannot be confirmed yet.
    pub fn replicas(&self) -> Option<u32> {
        match self {
            Self::Armed {
                action: Action::Scale,
                replicas,
            } => parse_replicas(replicas),
            _ => None,
        }
    }

    /// Whether the second gesture can be made: everything but a scale with
    /// nothing usable in its field.
    pub fn can_confirm(&self) -> bool {
        match self {
            Self::Idle => false,
            Self::Armed {
                action: Action::Scale,
                ..
            } => self.replicas().is_some(),
            Self::Armed { .. } => true,
        }
    }
}

/// A replica count as a person types it.
///
/// Whole, non-negative, and under a ceiling no real workload reaches — a
/// scale to a million replicas is a typo, and the apiserver would accept it.
pub fn parse_replicas(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>().ok().filter(|count| *count <= 10_000)
}

/// What the confirming button says.
///
/// Always the action *and the object's name*, and for a scale the count too,
/// so the second gesture cannot be made without reading what it is for.
pub fn confirm_label(action: Action, name: &str, replicas: Option<u32>) -> String {
    match action {
        Action::Sync => rust_i18n::t!("action.confirm.sync", name = name).to_string(),
        Action::Scale => rust_i18n::t!(
            "action.confirm.scale",
            name = name,
            replicas = replicas.unwrap_or_default()
        )
        .to_string(),
        Action::Restart => rust_i18n::t!("action.confirm.restart", name = name).to_string(),
        Action::Cordon => rust_i18n::t!("action.confirm.cordon", name = name).to_string(),
        Action::Uncordon => rust_i18n::t!("action.confirm.uncordon", name = name).to_string(),
        Action::Drain => rust_i18n::t!("action.confirm.drain", name = name).to_string(),
        Action::Apply => rust_i18n::t!("action.confirm.apply", name = name).to_string(),
        Action::Delete => rust_i18n::t!("action.confirm.delete", name = name).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_second_gesture_always_names_the_object() {
        rust_i18n::set_locale("en");
        for action in Action::ALL {
            let label = confirm_label(*action, "api-7d9f8c-2xk4t", Some(3));
            assert!(label.contains("api-7d9f8c-2xk4t"), "{action:?}: {label}");
        }
    }

    #[test]
    fn a_scale_names_the_count_as_well() {
        rust_i18n::set_locale("en");
        let label = confirm_label(Action::Scale, "api", Some(3));
        assert!(label.contains('3'), "{label}");
    }

    #[test]
    fn the_same_holds_in_japanese() {
        rust_i18n::set_locale("ja");
        for action in Action::ALL {
            let label = confirm_label(*action, "api-7d9f8c-2xk4t", Some(3));
            assert!(label.contains("api-7d9f8c-2xk4t"), "{action:?}: {label}");
        }
        rust_i18n::set_locale("en");
    }

    #[test]
    fn arming_a_scale_starts_from_what_the_object_has() {
        let pending = Pending::arm(Action::Scale, 2);
        assert_eq!(pending.replicas(), Some(2));
        assert!(pending.can_confirm());
        assert_eq!(pending.action(), Some(Action::Scale));
    }

    #[test]
    fn a_scale_with_nothing_usable_typed_cannot_be_confirmed() {
        let pending = Pending::Armed {
            action: Action::Scale,
            replicas: "lots".into(),
        };
        assert!(!pending.can_confirm());
        let pending = Pending::Armed {
            action: Action::Scale,
            replicas: String::new(),
        };
        assert!(!pending.can_confirm());
    }

    #[test]
    fn everything_else_can_be_confirmed_the_moment_it_is_armed() {
        for action in [
            Action::Sync,
            Action::Delete,
            Action::Restart,
            Action::Cordon,
            Action::Drain,
            Action::Apply,
        ] {
            assert!(Pending::arm(action, 1).can_confirm(), "{action:?}");
        }
        assert!(!Pending::Idle.can_confirm());
        assert_eq!(Pending::Idle.action(), None);
    }

    #[test]
    fn a_replica_count_is_whole_and_not_absurd() {
        assert_eq!(parse_replicas("3"), Some(3));
        assert_eq!(parse_replicas(" 0 "), Some(0));
        assert_eq!(parse_replicas("-1"), None);
        assert_eq!(parse_replicas("1.5"), None);
        assert_eq!(parse_replicas("1e3"), None);
        assert_eq!(parse_replicas(""), None);
        assert_eq!(parse_replicas("1000000"), None);
    }
}
