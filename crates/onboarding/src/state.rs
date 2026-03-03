//! Pure state machine for the onboarding wizard. No I/O.

use moltis_config::{AgentIdentity, UserProfile};

/// Steps in the onboarding wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WizardStep {
    Welcome,
    UserName,
    AgentName,
    AgentEmoji,
    AgentCreature,
    AgentVibe,
    Confirm,
    Done,
}

/// The wizard state, advanced one step at a time.
#[derive(Debug, Clone)]
pub struct WizardState {
    pub step: WizardStep,
    pub user: UserProfile,
    pub identity: AgentIdentity,
    /// Public display name for the default agent (stored in PEOPLE.md).
    pub agent_display_name: Option<String>,
}

impl Default for WizardState {
    fn default() -> Self {
        Self::new()
    }
}

impl WizardState {
    pub fn new() -> Self {
        Self {
            step: WizardStep::Welcome,
            user: UserProfile::default(),
            identity: AgentIdentity::default(),
            agent_display_name: None,
        }
    }

    /// The prompt text to display for the current step.
    pub fn prompt(&self) -> &str {
        match self.step {
            WizardStep::Welcome => {
                "Welcome to moltis! Let's set things up. Press Enter to continue."
            },
            WizardStep::UserName => "What's your name?",
            WizardStep::AgentName => "Pick a display name for your agent:",
            WizardStep::AgentEmoji => "Choose an emoji for your agent (e.g. \u{1f916}):",
            WizardStep::AgentCreature => {
                "What kind of creature is your agent? (e.g. owl, fox, dragon)"
            },
            WizardStep::AgentVibe => {
                "Describe your agent's vibe in a few words (e.g. chill, witty, formal):"
            },
            WizardStep::Confirm => "All set! Press Enter to save, or type 'back' to go back.",
            WizardStep::Done => "Onboarding complete!",
        }
    }

    /// Process user input and advance to the next step.
    pub fn advance(&mut self, input: &str) {
        let input = input.trim();
        match self.step {
            WizardStep::Welcome => self.step = WizardStep::UserName,
            WizardStep::UserName => {
                if !input.is_empty() {
                    self.user.name = Some(input.to_string());
                }
                self.step = WizardStep::AgentName;
            },
            WizardStep::AgentName => {
                if !input.is_empty() {
                    self.agent_display_name = Some(input.to_string());
                } else if self.agent_display_name.is_none() {
                    self.agent_display_name = Some("moltis".to_string());
                }
                self.step = WizardStep::AgentEmoji;
            },
            WizardStep::AgentEmoji => {
                if !input.is_empty() {
                    self.identity.emoji = Some(input.to_string());
                }
                self.step = WizardStep::AgentCreature;
            },
            WizardStep::AgentCreature => {
                if !input.is_empty() {
                    self.identity.creature = Some(input.to_string());
                }
                self.step = WizardStep::AgentVibe;
            },
            WizardStep::AgentVibe => {
                if !input.is_empty() {
                    self.identity.vibe = Some(input.to_string());
                }
                self.step = WizardStep::Confirm;
            },
            WizardStep::Confirm => {
                if input.eq_ignore_ascii_case("back") {
                    self.step = WizardStep::AgentVibe;
                } else {
                    self.step = WizardStep::Done;
                }
            },
            WizardStep::Done => {},
        }
    }

    pub fn is_done(&self) -> bool {
        self.step == WizardStep::Done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_wizard_flow() {
        let mut s = WizardState::new();
        assert_eq!(s.step, WizardStep::Welcome);

        s.advance(""); // welcome → user name
        assert_eq!(s.step, WizardStep::UserName);

        s.advance("Alice"); // → agent name
        assert_eq!(s.user.name.as_deref(), Some("Alice"));
        assert_eq!(s.step, WizardStep::AgentName);

        s.advance("Momo"); // → emoji
        assert_eq!(s.agent_display_name.as_deref(), Some("Momo"));

        s.advance("\u{1f99c}"); // → creature
        assert_eq!(s.identity.emoji.as_deref(), Some("\u{1f99c}"));

        s.advance("parrot"); // → vibe
        assert_eq!(s.identity.creature.as_deref(), Some("parrot"));

        s.advance("cheerful and curious"); // → confirm
        assert_eq!(s.identity.vibe.as_deref(), Some("cheerful and curious"));
        assert_eq!(s.step, WizardStep::Confirm);

        s.advance(""); // confirm → done
        assert!(s.is_done());
    }

    #[test]
    fn back_from_confirm() {
        let mut s = WizardState::new();
        // fast-forward to confirm
        s.advance(""); // welcome
        s.advance("Bob");
        s.advance("Rex");
        s.advance("\u{1f436}");
        s.advance("dog");
        s.advance("loyal");
        assert_eq!(s.step, WizardStep::Confirm);

        s.advance("back");
        assert_eq!(s.step, WizardStep::AgentVibe);
    }

    #[test]
    fn default_agent_name() {
        let mut s = WizardState::new();
        s.advance(""); // welcome
        s.advance("User");
        s.advance(""); // empty agent name → defaults to "moltis"
        assert_eq!(s.agent_display_name.as_deref(), Some("moltis"));
    }
}
