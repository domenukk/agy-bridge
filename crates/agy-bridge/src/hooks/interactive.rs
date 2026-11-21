//! Interactive question/answer types for agent-user interactions (`0.1.24`+).

use serde::{Deserialize, Serialize};

use super::types::HookResult;

/// A single selectable option in an interactive question prompt.
///
/// Maps to `google.antigravity.types.AskQuestionOption` (`id`, `text`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionOption {
    /// Option identifier (1-based index string `"1"`, `"2"`, … on `localharness`).
    #[serde(alias = "value")]
    pub id: String,
    /// Human-readable option text displayed to the user.
    #[serde(alias = "label")]
    pub text: String,
}

impl AskQuestionOption {
    /// Create a new option with `id` and `text`.
    #[must_use]
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
        }
    }
}

/// A question with its list of selectable options.
///
/// Maps to `google.antigravity.types.AskQuestionEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionEntry {
    /// The question text to present to the user.
    pub question: String,
    /// Available answer options.
    pub options: Vec<AskQuestionOption>,
    /// Whether the user may select multiple options.
    #[serde(default)]
    pub is_multi_select: bool,
}

impl AskQuestionEntry {
    /// Create a single-select question entry.
    #[must_use]
    pub fn new(question: impl Into<String>, options: Vec<AskQuestionOption>) -> Self {
        Self {
            question: question.into(),
            options,
            is_multi_select: false,
        }
    }
}

/// Full specification for an interactive `ask_question` dialog.
///
/// Maps to `google.antigravity.types.AskQuestionInteractionSpec`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestionInteractionSpec {
    /// Questions presented to the user in order.
    #[serde(alias = "entries", default)]
    pub questions: Vec<AskQuestionEntry>,
}

/// Individual response for an [`AskQuestionEntry`], matching `0.1.24` `QuestionResponse`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionResponse {
    /// Selected option IDs (e.g. `Some(vec!["1".into()])`).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "answers")]
    pub selected_option_ids: Option<Vec<String>>,
    /// Freeform text response when the user supplies custom text.
    #[serde(default)]
    pub freeform_response: String,
    /// If true, the question was skipped (`unanswered = true` on `localharness`).
    #[serde(default)]
    pub skipped: bool,
}

impl QuestionResponse {
    /// Build a response selecting one or more option IDs (e.g. `["1"]`).
    #[must_use]
    pub fn selected(ids: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            selected_option_ids: Some(ids.into_iter().map(Into::into).collect()),
            freeform_response: String::new(),
            skipped: false,
        }
    }

    /// Build a freeform text response.
    #[must_use]
    pub fn freeform(text: impl Into<String>) -> Self {
        Self {
            selected_option_ids: None,
            freeform_response: text.into(),
            skipped: false,
        }
    }

    /// Build a skipped response.
    #[must_use]
    pub const fn skipped() -> Self {
        Self {
            selected_option_ids: None,
            freeform_response: String::new(),
            skipped: true,
        }
    }
}

/// Result returned by [`super::Hooks::on_interaction`] callbacks (`0.1.24` `QuestionHookResult`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionHookResult {
    /// Responses corresponding to the requested questions.
    #[serde(default)]
    pub responses: Vec<QuestionResponse>,
    /// If true, the interaction was cancelled by the user/host.
    #[serde(default)]
    pub cancelled: bool,
}

impl QuestionHookResult {
    /// Construct an answered interaction result with one [`QuestionResponse`] per question.
    #[must_use]
    pub fn answered(responses: impl IntoIterator<Item = QuestionResponse>) -> Self {
        Self {
            responses: responses.into_iter().collect(),
            cancelled: false,
        }
    }

    /// Construct a cancelled interaction result.
    #[must_use]
    pub const fn cancelled() -> Self {
        Self {
            responses: Vec::new(),
            cancelled: true,
        }
    }
}

impl From<HookResult> for QuestionHookResult {
    fn from(res: HookResult) -> Self {
        if res.allow {
            Self::default()
        } else {
            Self::cancelled()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_question_option_serde_roundtrip() {
        let opt = AskQuestionOption::new("1", "Yes");
        let json = serde_json::to_string(&opt).unwrap();
        let parsed: AskQuestionOption = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, opt);
    }

    #[test]
    fn ask_question_entry_serde_roundtrip() {
        let entry = AskQuestionEntry {
            question: "Continue?".into(),
            options: vec![
                AskQuestionOption::new("1", "Yes"),
                AskQuestionOption::new("2", "No"),
            ],
            is_multi_select: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: AskQuestionEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, entry);
    }

    #[test]
    fn ask_question_interaction_spec_serde_roundtrip() {
        let spec = AskQuestionInteractionSpec {
            questions: vec![
                AskQuestionEntry::new("Q1?", vec![AskQuestionOption::new("1", "A")]),
                AskQuestionEntry::new("Q2?", vec![]),
            ],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: AskQuestionInteractionSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn question_response_and_result_serde_roundtrip() {
        let resp = QuestionResponse::selected(["1", "2"]);
        let result = QuestionHookResult::answered([resp.clone(), QuestionResponse::skipped()]);
        let json = serde_json::to_string(&result).unwrap();
        let parsed: QuestionHookResult = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, result);
        assert!(!parsed.cancelled);
        assert_eq!(parsed.responses[0], resp);
        assert!(parsed.responses[1].skipped);

        let cancelled: QuestionHookResult = HookResult::deny("stop").into();
        assert!(cancelled.cancelled);
    }
}
