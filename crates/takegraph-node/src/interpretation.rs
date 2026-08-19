//! Local interpretation of a transcript into intent candidates.
//!
//! The first slice is a deterministic heuristic plus a human correction
//! path. Capture audio is not mutated. An LLM is not bundled or called.

use takegraph_core::{
    AnnotationError, AnnotationId, AnnotationIntent, AnnotationInterpretation,
    AnnotationTranscript, TemporalReference, TemporalRelation, canonical_sha256,
};
use thiserror::Error;
use uuid::Uuid;

/// Versioned identity of the built-in phrase table. Changing the table must
/// change [`HEURISTIC_MODEL_ID`] or this version so digests move.
pub const HEURISTIC_MODEL_ID: &str = "heuristic-v1";
const HEURISTIC_TABLE_VERSION: u32 = 1;

/// Inputs available to one interpretation provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpretationInput {
    pub capture_id: AnnotationId,
    pub transcript_digest: String,
    pub text: String,
    pub start_frame: i32,
    pub end_frame: i32,
    pub fps: u32,
}

/// Unsealed candidate produced by a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpretationDraft {
    pub temporal: TemporalReference,
    pub intents: Vec<AnnotationIntent>,
}

/// Provider-side interpretation failure. This never deletes a capture or
/// transcript.
#[derive(Debug, Error)]
pub enum InterpretationError {
    #[error("interpretation input is unavailable: {0}")]
    Unavailable(String),
    #[error("interpretation produced no intents: {0}")]
    Empty(String),
    #[error("interpretation failed: {0}")]
    Failed(String),
    #[error(transparent)]
    Canonical(#[from] takegraph_core::CanonicalError),
    #[error(transparent)]
    Domain(#[from] AnnotationError),
}

/// One interpretation backend. Implementations must not mutate the store.
pub trait InterpretationProvider: Send + Sync {
    /// Stable model identity recorded on the interpretation.
    fn model_id(&self) -> &str;

    /// Digest over model identity and parameters.
    ///
    /// # Errors
    ///
    /// Returns [`InterpretationError::Unavailable`] when the model binding
    /// cannot be hashed.
    fn model_digest(&self) -> Result<String, InterpretationError>;

    /// Derives a temporal range and at least one intent from a transcript.
    ///
    /// # Errors
    ///
    /// Returns a provider failure. Empty intents are [`InterpretationError::Empty`].
    fn interpret(
        &self,
        input: &InterpretationInput,
    ) -> Result<InterpretationDraft, InterpretationError>;
}

/// Seals a validated interpretation bound to an exact transcript digest.
///
/// # Errors
///
/// Returns [`AnnotationError::InvalidInterpretation`] for empty intents or a
/// digest construction failure.
pub fn seal_interpretation(
    capture_id: AnnotationId,
    transcript: &AnnotationTranscript,
    temporal: TemporalReference,
    intents: Vec<AnnotationIntent>,
    model_id: impl Into<String>,
    model_digest: impl Into<String>,
) -> Result<AnnotationInterpretation, AnnotationError> {
    let model_id = model_id.into();
    let model_digest = model_digest.into();
    let interpretation_digest = canonical_sha256(
        "takegraph-annotation-interpretation-v1",
        &(
            capture_id,
            transcript.transcript_digest.as_str(),
            &temporal,
            &intents,
            model_id.as_str(),
            model_digest.as_str(),
        ),
    )
    .map_err(|error| AnnotationError::InvalidInterpretation(error.to_string()))?;
    let interpretation = AnnotationInterpretation {
        id: Uuid::new_v4(),
        capture_id,
        transcript_digest: transcript.transcript_digest.clone(),
        temporal,
        intents,
        model_id,
        model_digest,
        interpretation_digest,
    };
    interpretation.validate()?;
    Ok(interpretation)
}

/// Deterministic Japanese phrase table. Not an LLM.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicInterpreter;

impl HeuristicInterpreter {
    /// Parses one transcript against the capture anchors.
    #[must_use]
    pub fn draft(input: &InterpretationInput) -> InterpretationDraft {
        let fps = input.fps.max(1);
        let capture_span = (input.end_frame - input.start_frame).max(0);
        let temporal = parse_temporal(&input.text, input.start_frame, capture_span, fps);
        let intents = parse_intents(&input.text);
        InterpretationDraft { temporal, intents }
    }
}

impl InterpretationProvider for HeuristicInterpreter {
    fn model_id(&self) -> &str {
        HEURISTIC_MODEL_ID
    }

    fn model_digest(&self) -> Result<String, InterpretationError> {
        Ok(canonical_sha256(
            "takegraph-interpretation-model-v1",
            &(HEURISTIC_MODEL_ID, HEURISTIC_TABLE_VERSION),
        )?)
    }

    fn interpret(
        &self,
        input: &InterpretationInput,
    ) -> Result<InterpretationDraft, InterpretationError> {
        let draft = Self::draft(input);
        if draft.intents.is_empty() {
            return Err(InterpretationError::Empty(
                "heuristic produced no intents".into(),
            ));
        }
        draft.temporal.validate()?;
        for intent in &draft.intents {
            intent.validate()?;
        }
        Ok(draft)
    }
}

/// Human-supplied intents. Used for corrections and tests that skip the
/// phrase table.
#[derive(Debug, Clone)]
pub struct HumanInterpretation {
    reviewer: String,
    temporal: TemporalReference,
    intents: Vec<AnnotationIntent>,
}

impl HumanInterpretation {
    /// Builds a human correction. Empty intents fail at [`interpret`](Self::interpret).
    #[must_use]
    pub fn new(
        reviewer: impl Into<String>,
        temporal: TemporalReference,
        intents: Vec<AnnotationIntent>,
    ) -> Self {
        Self {
            reviewer: reviewer.into(),
            temporal,
            intents,
        }
    }
}

impl InterpretationProvider for HumanInterpretation {
    fn model_id(&self) -> &str {
        "human"
    }

    fn model_digest(&self) -> Result<String, InterpretationError> {
        Ok(canonical_sha256(
            "takegraph-interpretation-model-v1",
            &("human", self.reviewer.as_str()),
        )?)
    }

    fn interpret(
        &self,
        _input: &InterpretationInput,
    ) -> Result<InterpretationDraft, InterpretationError> {
        if self.intents.is_empty() {
            return Err(InterpretationError::Empty(
                "human interpretation requires at least one intent".into(),
            ));
        }
        self.temporal.validate()?;
        for intent in &self.intents {
            intent.validate()?;
        }
        Ok(InterpretationDraft {
            temporal: self.temporal.clone(),
            intents: self.intents.clone(),
        })
    }
}

fn parse_temporal(
    text: &str,
    reference_frame: i32,
    capture_span: i32,
    fps: u32,
) -> TemporalReference {
    let fps_i = i32::try_from(fps).unwrap_or(i32::MAX);
    if let Some(seconds) = first_seconds_before(text) {
        let start = -seconds.saturating_mul(fps_i);
        return TemporalReference {
            reference_frame,
            start_offset_frames: start,
            end_offset_frames: Some(0),
            relation: TemporalRelation::Range,
        };
    }
    if text.contains("ここ全部") {
        return TemporalReference {
            reference_frame,
            start_offset_frames: 0,
            end_offset_frames: Some(capture_span),
            relation: TemporalRelation::Range,
        };
    }
    if text.contains("ここから") || text.contains("この後") {
        return TemporalReference {
            reference_frame,
            start_offset_frames: 0,
            end_offset_frames: None,
            relation: TemporalRelation::After,
        };
    }
    if text.contains("さっき") {
        return TemporalReference {
            reference_frame,
            start_offset_frames: -3i32.saturating_mul(fps_i),
            end_offset_frames: Some(0),
            relation: TemporalRelation::Before,
        };
    }
    TemporalReference {
        reference_frame,
        start_offset_frames: 0,
        end_offset_frames: None,
        relation: TemporalRelation::At,
    }
}

fn parse_intents(text: &str) -> Vec<AnnotationIntent> {
    let mut intents = Vec::new();
    if contains_any(text, &["残す", "残して", "キープ"]) {
        intents.push(AnnotationIntent::Highlight {
            reason: Some("残す".into()),
        });
    }
    if contains_any(text, &["切る", "カット", "いらない", "要らない"]) {
        intents.push(AnnotationIntent::CutCandidate {
            reason: Some("切る".into()),
        });
    }
    if contains_any(text, &["確認", "チェック", "あってる"]) {
        intents.push(AnnotationIntent::Verify {
            question: first_sentence(text),
        });
    }
    if contains_any(text, &["説明", "ナレーション", "台詞"]) {
        intents.push(AnnotationIntent::Narration {
            topic: narration_topic(text),
            draft_hint: Some(text.trim().to_owned()),
        });
    }
    if intents.is_empty() {
        intents.push(AnnotationIntent::Note);
    }
    intents
}

fn first_seconds_before(text: &str) -> Option<i32> {
    let markers = ["秒前から", "秒前"];
    for marker in markers {
        if let Some(index) = text.find(marker) {
            let prefix = &text[..index];
            let raw: String = prefix
                .chars()
                .rev()
                .take_while(|ch| is_count_char(*ch))
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if !raw.is_empty() {
                if let Some(value) = parse_count(&raw) {
                    return Some(i32::try_from(value).unwrap_or(i32::MAX));
                }
            }
        }
    }
    None
}

fn is_count_char(ch: char) -> bool {
    ch.is_ascii_digit()
        || ('０'..='９').contains(&ch)
        || matches!(
            ch,
            '一' | '二' | '三' | '四' | '五' | '六' | '七' | '八' | '九' | '十'
        )
}

fn parse_count(raw: &str) -> Option<u32> {
    match raw {
        "一" => Some(1),
        "二" => Some(2),
        "三" => Some(3),
        "四" => Some(4),
        "五" => Some(5),
        "六" => Some(6),
        "七" => Some(7),
        "八" => Some(8),
        "九" => Some(9),
        "十" => Some(10),
        _ => {
            let ascii: String = raw
                .chars()
                .map(|ch| {
                    if ('０'..='９').contains(&ch) {
                        char::from_u32(u32::from(ch) - u32::from('０') + u32::from('0'))
                            .unwrap_or(ch)
                    } else {
                        ch
                    }
                })
                .collect();
            ascii.parse().ok()
        }
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    trimmed
        .split_once(['。', '！', '？', '\n'])
        .map(|(head, _)| head.trim())
        .filter(|head| !head.is_empty())
        .unwrap_or(trimmed)
        .to_owned()
}

fn narration_topic(text: &str) -> String {
    let latin: String = text
        .chars()
        .skip_while(|ch| !ch.is_ascii_alphabetic())
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(*ch, ' ' | '_' | '-'))
        .collect::<String>()
        .trim()
        .to_owned();
    if !latin.is_empty() {
        return latin;
    }
    let stripped = text
        .replace("ここは", "")
        .replace("これは", "")
        .replace("説明を入れる", "")
        .replace("説明", "");
    let compact: String = stripped
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .take(20)
        .collect();
    if compact.is_empty() {
        "untagged".into()
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript(text: &str) -> AnnotationTranscript {
        AnnotationTranscript {
            id: Uuid::new_v4(),
            capture_id: AnnotationId::new(),
            audio_sha256: format!("sha256:{}", "a".repeat(64)),
            text: text.into(),
            provider_id: "whisper-cpp".into(),
            provider_digest: format!("sha256:{}", "b".repeat(64)),
            transcript_digest: format!("sha256:{}", "c".repeat(64)),
        }
    }

    fn input(text: &str) -> InterpretationInput {
        InterpretationInput {
            capture_id: AnnotationId::new(),
            transcript_digest: format!("sha256:{}", "c".repeat(64)),
            text: text.into(),
            start_frame: 2531,
            end_frame: 2698,
            fps: 60,
        }
    }

    #[test]
    fn design_example_yields_range_highlight_and_narration() {
        let draft = HeuristicInterpreter
            .interpret(&input(
                "今のところ三秒前から残す。ここはCompressionの説明を入れる",
            ))
            .unwrap();
        assert_eq!(draft.temporal.reference_frame, 2531);
        assert_eq!(draft.temporal.relation, TemporalRelation::Range);
        assert_eq!(draft.temporal.start_offset_frames, -180);
        assert_eq!(draft.temporal.end_offset_frames, Some(0));
        assert!(
            draft
                .intents
                .iter()
                .any(|intent| matches!(intent, AnnotationIntent::Highlight { .. }))
        );
        assert!(draft.intents.iter().any(|intent| matches!(
            intent,
            AnnotationIntent::Narration { topic, .. } if topic == "Compression"
        )));
    }

    #[test]
    fn unmarked_speech_is_a_note_at_the_start_frame() {
        let draft = HeuristicInterpreter
            .interpret(&input("とりあえずメモ"))
            .unwrap();
        assert_eq!(draft.temporal.relation, TemporalRelation::At);
        assert_eq!(draft.intents, vec![AnnotationIntent::Note]);
    }

    #[test]
    fn sealed_digest_moves_when_intents_change() {
        let capture = AnnotationId::new();
        let spoken = transcript("残す");
        let first = seal_interpretation(
            capture,
            &spoken,
            TemporalReference {
                reference_frame: 10,
                start_offset_frames: 0,
                end_offset_frames: None,
                relation: TemporalRelation::At,
            },
            vec![AnnotationIntent::Note],
            "heuristic-v1",
            format!("sha256:{}", "d".repeat(64)),
        )
        .unwrap();
        let second = seal_interpretation(
            capture,
            &spoken,
            TemporalReference {
                reference_frame: 10,
                start_offset_frames: 0,
                end_offset_frames: None,
                relation: TemporalRelation::At,
            },
            vec![AnnotationIntent::Highlight {
                reason: Some("残す".into()),
            }],
            "heuristic-v1",
            format!("sha256:{}", "d".repeat(64)),
        )
        .unwrap();
        assert_ne!(first.interpretation_digest, second.interpretation_digest);
        assert_eq!(first.transcript_digest, spoken.transcript_digest);
    }
}
