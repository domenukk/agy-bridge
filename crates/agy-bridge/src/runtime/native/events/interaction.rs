//! User input encoding (with media descriptions and C0 control stripping) and
//! interactive `ask_question` (`UserQuestionsRequest` -> `UserQuestionsResponse`) handling.

use tokio::sync::mpsc;

use crate::{
    agent::AgentId,
    content::{Content, ContentPrimitive, MediaContent},
    hooks::{AskQuestionEntry, AskQuestionOption, OnInteractionContext, QuestionResponse},
    proto,
    runtime::bridge_state::agent_hooks,
};

fn sanitize_user_text(raw: &str) -> String {
    if raw
        .chars()
        .all(|c| c == '\n' || c == '\r' || c == '\t' || c >= '\x20')
    {
        return raw.to_owned();
    }
    raw.chars()
        .filter(|&c| c == '\n' || c == '\r' || c == '\t' || c >= '\x20')
        .collect()
}

fn to_proto_media_part<M: MediaContent>(media: &M) -> proto::localharness::user_input::Part {
    let description = match media.description() {
        Some(d) => d.to_owned(),
        None => String::new(),
    };
    proto::localharness::user_input::Part {
        part: Some(proto::localharness::user_input::part::Part::Media(
            proto::localharness::user_input::Media {
                mime_type: media.mime_type().to_owned(),
                description,
                data: media.data().to_vec(),
            },
        )),
    }
}

fn to_proto_text_part(text: &str) -> proto::localharness::user_input::Part {
    proto::localharness::user_input::Part {
        part: Some(proto::localharness::user_input::part::Part::Text(
            sanitize_user_text(text),
        )),
    }
}

fn to_proto_primitive_part(p: &ContentPrimitive) -> proto::localharness::user_input::Part {
    match p {
        ContentPrimitive::Text { text } => to_proto_text_part(text),
        ContentPrimitive::Image(img) => to_proto_media_part(img),
        ContentPrimitive::Audio(audio) => to_proto_media_part(audio),
        ContentPrimitive::Document(doc) => to_proto_media_part(doc),
        ContentPrimitive::Video(video) => to_proto_media_part(video),
    }
}

pub(crate) fn to_proto_user_input(content: &Content) -> proto::localharness::UserInput {
    let parts = match content {
        Content::Text { text } => vec![to_proto_text_part(text)],
        Content::Image(img) => vec![to_proto_media_part(img)],
        Content::Audio(audio) => vec![to_proto_media_part(audio)],
        Content::Document(doc) => vec![to_proto_media_part(doc)],
        Content::Video(video) => vec![to_proto_media_part(video)],
        Content::Multi { parts: primitives } => {
            primitives.iter().map(to_proto_primitive_part).collect()
        }
    };
    proto::localharness::UserInput { parts }
}

fn response_to_proto_answer(resp: &QuestionResponse) -> proto::localharness::UserQuestionAnswer {
    if resp.skipped {
        return proto::localharness::UserQuestionAnswer {
            answer: Some(proto::localharness::user_question_answer::Answer::Unanswered(true)),
        };
    }
    let mut selected_choice_indices = Vec::new();
    if let Some(ref ids) = resp.selected_option_ids {
        for id in ids {
            match id.trim().parse::<i32>() {
                Ok(one_based) if one_based >= 1 => {
                    selected_choice_indices.push(one_based - 1);
                }
                Ok(0) => {
                    selected_choice_indices.push(0);
                }
                Ok(neg) => {
                    tracing::debug!(option_id = %id, parsed = neg, "Ignoring negative selected_option_id");
                }
                Err(err) => {
                    tracing::debug!(option_id = %id, error = %err, "Ignoring non-numeric selected_option_id");
                }
            }
        }
    }
    if selected_choice_indices.is_empty() && resp.freeform_response.is_empty() {
        return proto::localharness::UserQuestionAnswer {
            answer: Some(proto::localharness::user_question_answer::Answer::Unanswered(true)),
        };
    }
    proto::localharness::UserQuestionAnswer {
        answer: Some(
            proto::localharness::user_question_answer::Answer::MultipleChoiceAnswer(
                proto::localharness::MultipleChoiceAnswer {
                    selected_choice_indices,
                    freeform_response: resp.freeform_response.clone(),
                },
            ),
        ),
    }
}

pub(crate) async fn handle_questions_request(
    agent_id: AgentId,
    step_update: &proto::localharness::StepUpdate,
    req: &proto::localharness::UserQuestionsRequest,
    event_tx: &mpsc::Sender<proto::localharness::InputEvent>,
) {
    let mut entries = Vec::new();
    let mut indices_to_hook = Vec::new();
    for (idx, uq) in req.questions.iter().enumerate() {
        if let Some(proto::localharness::user_question::QuestionType::MultipleChoice(ref mc)) =
            uq.question_type
        {
            let options = mc
                .choices
                .iter()
                .enumerate()
                .map(|(j, choice)| AskQuestionOption::new((j + 1).to_string(), choice.clone()))
                .collect();
            entries.push(AskQuestionEntry {
                question: mc.question.clone(),
                options,
                is_multi_select: mc.is_multi_select,
            });
            indices_to_hook.push(idx);
        }
    }

    let result = match agent_hooks(agent_id) {
        Some(hr) if hr.has_on_interaction() && !entries.is_empty() => {
            let ctx = OnInteractionContext::from_questions(entries);
            let hook_res = tokio::task::spawn_blocking(move || hr.run_on_interaction(&ctx)).await;
            match hook_res {
                Ok(qres) if qres.cancelled => {
                    proto::localharness::user_questions_response::Result::Cancelled(true)
                }
                Ok(qres) => {
                    let mut answers: Vec<proto::localharness::UserQuestionAnswer> = req
                        .questions
                        .iter()
                        .map(|_q| proto::localharness::UserQuestionAnswer {
                            answer: Some(
                                proto::localharness::user_question_answer::Answer::Unanswered(true),
                            ),
                        })
                        .collect();
                    for (orig_idx, r) in indices_to_hook.into_iter().zip(qres.responses.iter()) {
                        answers[orig_idx] = response_to_proto_answer(r);
                    }
                    proto::localharness::user_questions_response::Result::Response(
                        proto::localharness::user_questions_response::QuestionsResponse { answers },
                    )
                }
                Err(err) => {
                    tracing::error!(error = %err, "on_interaction hook panicked in spawn_blocking");
                    proto::localharness::user_questions_response::Result::Cancelled(true)
                }
            }
        }
        _ => proto::localharness::user_questions_response::Result::Cancelled(true),
    };

    let resp_event = proto::localharness::InputEvent {
        event: Some(proto::localharness::input_event::Event::QuestionResponse(
            proto::localharness::UserQuestionsResponse {
                trajectory_id: step_update.trajectory_id.clone(),
                step_index: step_update.step_index,
                result: Some(result),
            },
        )),
    };
    if let Err(err) = event_tx.send(resp_event).await {
        tracing::warn!(error = %err, "Failed sending UserQuestionsResponse to localharness");
    }
}
