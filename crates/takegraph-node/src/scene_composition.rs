//! Typed, read-only observation of the currently evaluated YMM4 scene.
//!
//! This contract deliberately represents unavailable visual state explicitly.
//! In particular, consumers must not infer geometry from timeline placement or
//! native-extension owned fields.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use takegraph_core::ItemKind;

use crate::{Ymm4BridgeClient, Ymm4Error};

/// Maps a YMM4 composition `kind` string onto the portable item family.
///
/// The bridge reports type suffixes without the `Item` tail (`Voice`, `Text`).
/// Unknown plugin or tachie-specific labels stay [`ItemKind::UnknownItem`] so
/// composition-graph mutation can fail closed instead of guessing.
#[must_use]
pub fn parse_composition_item_kind(kind: &str) -> ItemKind {
    match kind {
        "Audio" | "AudioItem" => ItemKind::AudioItem,
        "Voice" | "VoiceItem" => ItemKind::VoiceItem,
        "Annotation"
        | "AnnotationItem"
        | "TakeGraphAnnotationItem"
        | "annotation"
        | "takegraphannotation" => ItemKind::AnnotationItem,
        "Image" | "ImageItem" | "Video" | "VideoItem" => ItemKind::AssetItem,
        "Text" | "TextItem" => ItemKind::TextItem,
        "Shape" | "ShapeItem" => ItemKind::ShapeItem,
        "Effect" | "EffectItem" => ItemKind::EffectItem,
        "FrameBuffer" | "FrameBufferItem" => ItemKind::FrameBufferItem,
        "Group" | "GroupItem" => ItemKind::GroupItem,
        "Scene" | "SceneItem" | "SceneTimeline" => ItemKind::SceneItem,
        "Transition" | "TransitionItem" => ItemKind::TransitionItem,
        _ => ItemKind::UnknownItem,
    }
}

fn require_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Schema version of [`Ymm4SceneCompositionSnapshot`].
pub const YMM4_SCENE_COMPOSITION_SCHEMA_VERSION: u32 = 1;

/// Whether a fixed set of integer observation fields was available from YMM4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4CompositionAvailability {
    Available,
    Unavailable,
}

/// Stability guarantee of an element identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4CompositionElementStability {
    /// Derived from a persisted `TakeGraph` realization identity.
    RealizationIdentity,
    /// Deterministic only within the current YMM4 session/snapshot source.
    SessionOnly,
}

/// How completely the bridge could observe the current scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ymm4CompositionCompleteness {
    Complete,
    Partial,
}

/// Evaluated viewport dimensions in integer pixels, or explicit placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CompositionViewport {
    pub availability: Ymm4CompositionAvailability,
    #[serde(deserialize_with = "require_present_option")]
    pub width: Option<i32>,
    #[serde(deserialize_with = "require_present_option")]
    pub height: Option<i32>,
}

impl Ymm4CompositionViewport {
    fn validate(&self) -> Result<(), Ymm4SceneCompositionError> {
        validate_dimensions("viewport", self.availability, self.width, self.height)
    }
}

/// Evaluated element rectangle in integer pixels, or explicit placeholders.
///
/// Negative `x` and `y` values are valid because an element may be partially
/// outside the viewport. Available widths and heights are always positive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CompositionVisual {
    pub availability: Ymm4CompositionAvailability,
    #[serde(deserialize_with = "require_present_option")]
    pub x: Option<i32>,
    #[serde(deserialize_with = "require_present_option")]
    pub y: Option<i32>,
    #[serde(deserialize_with = "require_present_option")]
    pub width: Option<i32>,
    #[serde(deserialize_with = "require_present_option")]
    pub height: Option<i32>,
}

impl Ymm4CompositionVisual {
    fn validate(&self, element_id: &str) -> Result<(), Ymm4SceneCompositionError> {
        match self.availability {
            Ymm4CompositionAvailability::Available => {
                if self.x.is_none() || self.y.is_none() {
                    return Err(Ymm4SceneCompositionError::InvalidField {
                        field: format!("elements[{element_id}].visual"),
                        reason: "available geometry requires integer x and y".into(),
                    });
                }
                validate_dimensions(
                    &format!("elements[{element_id}].visual"),
                    self.availability,
                    self.width,
                    self.height,
                )
            }
            Ymm4CompositionAvailability::Unavailable => {
                if self.x.is_some()
                    || self.y.is_some()
                    || self.width.is_some()
                    || self.height.is_some()
                {
                    return Err(Ymm4SceneCompositionError::InvalidField {
                        field: format!("elements[{element_id}].visual"),
                        reason: "unavailable geometry must contain only null placeholders".into(),
                    });
                }
                Ok(())
            }
        }
    }
}

/// One current-frame timeline element plus safely observable visual state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4CompositionElement {
    pub element_id: String,
    pub stability: Ymm4CompositionElementStability,
    pub kind: String,
    pub frame: i32,
    pub layer: i32,
    pub length: i32,
    pub active: bool,
    /// Selection is transient editor context. `None` means the host did not
    /// expose it for this element; it is not equivalent to unselected.
    #[serde(deserialize_with = "require_present_option")]
    pub selected: Option<bool>,
    /// Safely reflectable display text. Absence is represented as JSON `null`;
    /// when the adapter intentionally withholds text, `unavailableFields` also
    /// contains `elements[].text`.
    #[serde(deserialize_with = "require_present_option")]
    pub text: Option<String>,
    pub visual: Ymm4CompositionVisual,
}

impl Ymm4CompositionElement {
    fn validate(&self, current_frame: i32) -> Result<(), Ymm4SceneCompositionError> {
        require_non_empty_ascii(&self.element_id, "elements[].elementId")?;
        require_non_empty(&self.kind, "elements[].kind")?;
        if self.frame < 0 {
            return Err(invalid_field("elements[].frame", "must be zero or greater"));
        }
        if self.layer < 0 {
            return Err(invalid_field("elements[].layer", "must be zero or greater"));
        }
        if self.length <= 0 {
            return Err(invalid_field(
                "elements[].length",
                "must be greater than zero",
            ));
        }
        if self
            .text
            .as_ref()
            .is_some_and(|text| text.trim().is_empty())
        {
            return Err(invalid_field(
                "elements[].text",
                "empty text must be normalized to null",
            ));
        }

        let start = i64::from(self.frame);
        let end = start + i64::from(self.length);
        let current = i64::from(current_frame);
        let expected_active = start <= current && current < end;
        if !self.active || !expected_active {
            return Err(Ymm4SceneCompositionError::InvalidField {
                field: format!("elements[{}].active", self.element_id),
                reason: "current-frame snapshots may contain only active elements whose frame range includes the current frame".into(),
            });
        }
        self.visual.validate(&self.element_id)
    }
}

/// Read-only, current-frame composition observation returned by the bridge.
///
/// Elements are strictly ordered by `(layer, elementId)`. This makes the wire
/// representation deterministic without claiming that the order is YMM4 paint
/// order. `unavailableFields` is likewise sorted and duplicate-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ymm4SceneCompositionSnapshot {
    pub schema_version: u32,
    pub project_id: String,
    pub scene_id: String,
    pub source_fingerprint: String,
    pub fps: u32,
    pub frame: i32,
    pub viewport: Ymm4CompositionViewport,
    pub elements: Vec<Ymm4CompositionElement>,
    pub completeness: Ymm4CompositionCompleteness,
    pub unavailable_fields: Vec<String>,
}

impl Ymm4SceneCompositionSnapshot {
    /// Validates schema, source identity, availability/null invariants, element
    /// activity, deterministic ordering, and completeness metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is ambiguous or non-deterministic.
    #[allow(clippy::too_many_lines)] // One pass validates the complete cross-runtime wire contract.
    pub fn validate(&self) -> Result<(), Ymm4SceneCompositionError> {
        if self.schema_version != YMM4_SCENE_COMPOSITION_SCHEMA_VERSION {
            return Err(Ymm4SceneCompositionError::SchemaVersion {
                expected: YMM4_SCENE_COMPOSITION_SCHEMA_VERSION,
                actual: self.schema_version,
            });
        }
        require_non_empty(&self.project_id, "projectId")?;
        require_non_empty(&self.scene_id, "sceneId")?;
        require_non_empty_ascii(&self.source_fingerprint, "sourceFingerprint")?;
        if self.fps == 0 {
            return Err(invalid_field("fps", "must be greater than zero"));
        }
        if self.frame < 0 {
            return Err(invalid_field("frame", "must be zero or greater"));
        }
        self.viewport.validate()?;

        let mut ids = BTreeSet::new();
        let mut previous: Option<(i32, &str)> = None;
        for element in &self.elements {
            element.validate(self.frame)?;
            if !ids.insert(element.element_id.as_str()) {
                return Err(Ymm4SceneCompositionError::DuplicateElementId(
                    element.element_id.clone(),
                ));
            }
            let key = (element.layer, element.element_id.as_str());
            if previous.is_some_and(|previous| previous >= key) {
                return Err(Ymm4SceneCompositionError::ElementOrder {
                    previous: previous
                        .map(|(layer, id)| format!("({layer}, {id})"))
                        .unwrap_or_default(),
                    current: format!("({}, {})", element.layer, element.element_id),
                });
            }
            previous = Some(key);
        }

        let mut previous_field: Option<&str> = None;
        for field in &self.unavailable_fields {
            require_field_path(field)?;
            if previous_field.is_some_and(|previous| previous >= field.as_str()) {
                return Err(Ymm4SceneCompositionError::UnavailableFieldOrder {
                    previous: previous_field.unwrap_or_default().into(),
                    current: field.clone(),
                });
            }
            previous_field = Some(field);
        }
        match self.completeness {
            Ymm4CompositionCompleteness::Complete if !self.unavailable_fields.is_empty() => {
                return Err(invalid_field(
                    "completeness",
                    "complete snapshots cannot list unavailable fields",
                ));
            }
            Ymm4CompositionCompleteness::Partial if self.unavailable_fields.is_empty() => {
                return Err(invalid_field(
                    "completeness",
                    "partial snapshots must identify at least one unavailable field",
                ));
            }
            _ => {}
        }
        if self.completeness == Ymm4CompositionCompleteness::Complete
            && (self.viewport.availability == Ymm4CompositionAvailability::Unavailable
                || self.elements.iter().any(|element| {
                    element.visual.availability == Ymm4CompositionAvailability::Unavailable
                }))
        {
            return Err(invalid_field(
                "completeness",
                "complete snapshots cannot contain unavailable viewport or visual state",
            ));
        }
        let unavailable: BTreeSet<&str> =
            self.unavailable_fields.iter().map(String::as_str).collect();
        if self.viewport.availability == Ymm4CompositionAvailability::Unavailable
            && !unavailable.contains("viewport")
        {
            return Err(invalid_field(
                "unavailableFields",
                "unavailable viewport must be declared as viewport",
            ));
        }
        if self
            .elements
            .iter()
            .any(|element| element.visual.availability == Ymm4CompositionAvailability::Unavailable)
            && !unavailable.contains("elements[].visual")
        {
            return Err(invalid_field(
                "unavailableFields",
                "unavailable element geometry must be declared as elements[].visual",
            ));
        }
        let has_unknown_selection = self
            .elements
            .iter()
            .any(|element| element.selected.is_none());
        if has_unknown_selection && !unavailable.contains("elements[].selected") {
            return Err(invalid_field(
                "unavailableFields",
                "unknown element selection must be declared as elements[].selected",
            ));
        }
        if unavailable.contains("elements[].text")
            && self.elements.iter().all(|element| element.text.is_some())
        {
            return Err(invalid_field(
                "unavailableFields",
                "elements[].text may be unavailable only when at least one text value is null",
            ));
        }
        Ok(())
    }
}

impl Ymm4BridgeClient {
    /// Reads a typed observation of the active scene at YMM4's current frame.
    ///
    /// The route never seeks the playhead or mutates the project. The response
    /// is rejected unless its availability and deterministic-order invariants
    /// hold and its project matches an optional client project binding.
    ///
    /// # Errors
    ///
    /// Returns a transport, authentication, project-binding, deserialization,
    /// or composition-contract error.
    pub async fn current_scene_composition(
        &self,
    ) -> Result<Ymm4SceneCompositionSnapshot, Ymm4Error> {
        let snapshot: Ymm4SceneCompositionSnapshot =
            self.get_json("v2/scene/composition/current").await?;
        snapshot.validate()?;
        self.require_expected_project_id(&snapshot.project_id)?;
        Ok(snapshot)
    }
}

fn validate_dimensions(
    field: &str,
    availability: Ymm4CompositionAvailability,
    width: Option<i32>,
    height: Option<i32>,
) -> Result<(), Ymm4SceneCompositionError> {
    match availability {
        Ymm4CompositionAvailability::Available
            if width.is_some_and(|value| value > 0) && height.is_some_and(|value| value > 0) =>
        {
            Ok(())
        }
        Ymm4CompositionAvailability::Available => Err(Ymm4SceneCompositionError::InvalidField {
            field: field.into(),
            reason: "available dimensions require positive integer width and height".into(),
        }),
        Ymm4CompositionAvailability::Unavailable if width.is_none() && height.is_none() => Ok(()),
        Ymm4CompositionAvailability::Unavailable => Err(Ymm4SceneCompositionError::InvalidField {
            field: field.into(),
            reason: "unavailable dimensions must contain null width and height".into(),
        }),
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<(), Ymm4SceneCompositionError> {
    if value.trim().is_empty() {
        return Err(invalid_field(field, "must not be empty"));
    }
    Ok(())
}

fn require_non_empty_ascii(value: &str, field: &str) -> Result<(), Ymm4SceneCompositionError> {
    require_non_empty(value, field)?;
    if !value.is_ascii() || value.chars().any(char::is_whitespace) {
        return Err(invalid_field(
            field,
            "must be non-whitespace ASCII for portable deterministic ordering",
        ));
    }
    Ok(())
}

fn require_field_path(value: &str) -> Result<(), Ymm4SceneCompositionError> {
    require_non_empty_ascii(value, "unavailableFields[]")?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'[' | b']' | b'_'))
    {
        return Err(invalid_field(
            "unavailableFields[]",
            "must be a portable dotted ASCII field path",
        ));
    }
    Ok(())
}

fn invalid_field(field: &str, reason: &str) -> Ymm4SceneCompositionError {
    Ymm4SceneCompositionError::InvalidField {
        field: field.into(),
        reason: reason.into(),
    }
}

/// A bridge response violated the typed composition observation contract.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Ymm4SceneCompositionError {
    #[error("scene composition schema mismatch: expected {expected}, got {actual}")]
    SchemaVersion { expected: u32, actual: u32 },
    #[error("invalid scene composition field {field}: {reason}")]
    InvalidField { field: String, reason: String },
    #[error("duplicate scene composition elementId: {0}")]
    DuplicateElementId(String),
    #[error(
        "scene composition elements are not strictly ordered by (layer, elementId): {previous} before {current}"
    )]
    ElementOrder { previous: String, current: String },
    #[error(
        "scene composition unavailableFields are not strictly ordered and unique: {previous} before {current}"
    )]
    UnavailableFieldOrder { previous: String, current: String },
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use takegraph_core::ItemKind;

    #[test]
    fn composition_kind_parser_is_explicit_and_fail_closed() {
        assert_eq!(parse_composition_item_kind("Voice"), ItemKind::VoiceItem);
        assert_eq!(
            parse_composition_item_kind("TakeGraphAnnotationItem"),
            ItemKind::AnnotationItem
        );
        assert_eq!(
            parse_composition_item_kind("annotation"),
            ItemKind::AnnotationItem
        );
        assert_eq!(parse_composition_item_kind("TextItem"), ItemKind::TextItem);
        assert_eq!(parse_composition_item_kind("Shape"), ItemKind::ShapeItem);
        assert_eq!(parse_composition_item_kind("Image"), ItemKind::AssetItem);
        assert_eq!(
            parse_composition_item_kind("FrameBuffer"),
            ItemKind::FrameBufferItem
        );
        assert_eq!(parse_composition_item_kind("Tachie"), ItemKind::UnknownItem);
        assert_eq!(
            parse_composition_item_kind("vendor.plugin.Glow"),
            ItemKind::UnknownItem
        );
    }

    fn unavailable_visual() -> Ymm4CompositionVisual {
        Ymm4CompositionVisual {
            availability: Ymm4CompositionAvailability::Unavailable,
            x: None,
            y: None,
            width: None,
            height: None,
        }
    }

    fn element(id: &str, layer: i32) -> Ymm4CompositionElement {
        Ymm4CompositionElement {
            element_id: id.into(),
            stability: Ymm4CompositionElementStability::RealizationIdentity,
            kind: "voice".into(),
            frame: 100,
            layer,
            length: 60,
            active: true,
            selected: None,
            text: Some("現在の表示テキスト".into()),
            visual: unavailable_visual(),
        }
    }

    fn partial_snapshot() -> Ymm4SceneCompositionSnapshot {
        Ymm4SceneCompositionSnapshot {
            schema_version: YMM4_SCENE_COMPOSITION_SCHEMA_VERSION,
            project_id: "project-a".into(),
            scene_id: "scene-a".into(),
            source_fingerprint: "sha256:source-a".into(),
            fps: 60,
            frame: 120,
            viewport: Ymm4CompositionViewport {
                availability: Ymm4CompositionAvailability::Unavailable,
                width: None,
                height: None,
            },
            elements: vec![element("realization-01", 4)],
            completeness: Ymm4CompositionCompleteness::Partial,
            unavailable_fields: vec![
                "elements[].selected".into(),
                "elements[].visual".into(),
                "viewport".into(),
            ],
        }
    }

    #[test]
    fn validates_explicit_unavailable_geometry_and_unicode_text() {
        let snapshot = partial_snapshot();
        snapshot.validate().unwrap();
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert_eq!(json["elements"][0]["text"], "現在の表示テキスト");
        assert_eq!(
            json["elements"][0]["visual"]["width"],
            serde_json::Value::Null
        );
        assert!(json.get("schema_version").is_none());
    }

    #[test]
    fn accepts_explicitly_withheld_element_text() {
        let mut snapshot = partial_snapshot();
        snapshot.elements[0].text = None;
        snapshot
            .unavailable_fields
            .insert(1, "elements[].text".into());
        snapshot.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap()["elements"][0]["text"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn rejects_ambiguous_geometry_placeholders() {
        let mut snapshot = partial_snapshot();
        snapshot.viewport.width = Some(1920);
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. }) if field == "viewport"
        ));

        let mut snapshot = partial_snapshot();
        snapshot.elements[0].visual.availability = Ymm4CompositionAvailability::Available;
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "elements[realization-01].visual"
        ));

        let mut snapshot = partial_snapshot();
        snapshot
            .unavailable_fields
            .retain(|field| field != "viewport");
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "unavailableFields"
        ));

        let mut snapshot = partial_snapshot();
        snapshot
            .unavailable_fields
            .retain(|field| field != "elements[].selected");
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "unavailableFields"
        ));

        let mut snapshot = partial_snapshot();
        snapshot
            .unavailable_fields
            .retain(|field| field != "elements[].visual");
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "unavailableFields"
        ));
    }

    #[test]
    fn rejects_non_deterministic_element_and_unavailable_field_order() {
        let mut snapshot = partial_snapshot();
        snapshot.elements = vec![element("realization-b", 4), element("realization-a", 4)];
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::ElementOrder { .. })
        ));

        let mut snapshot = partial_snapshot();
        snapshot.unavailable_fields.reverse();
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::UnavailableFieldOrder { .. })
        ));
    }

    #[test]
    fn rejects_activity_that_does_not_match_the_current_frame() {
        let mut snapshot = partial_snapshot();
        snapshot.elements[0].active = false;
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "elements[realization-01].active"
        ));

        let mut snapshot = partial_snapshot();
        snapshot.elements[0].frame = 200;
        snapshot.elements[0].active = false;
        assert!(matches!(
            snapshot.validate(),
            Err(Ymm4SceneCompositionError::InvalidField { field, .. })
                if field == "elements[realization-01].active"
        ));
    }

    #[test]
    fn strict_wire_contract_rejects_unknown_fields() {
        let mut json = serde_json::to_value(partial_snapshot()).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("guessedBounds".into(), serde_json::json!({}));
        assert!(serde_json::from_value::<Ymm4SceneCompositionSnapshot>(json).is_err());

        for path in [
            &["viewport", "width"][..],
            &["elements", "0", "selected"][..],
            &["elements", "0", "text"][..],
            &["elements", "0", "visual", "x"][..],
        ] {
            let mut json = serde_json::to_value(partial_snapshot()).unwrap();
            let mut target = &mut json;
            for segment in &path[..path.len() - 1] {
                target = if let Ok(index) = segment.parse::<usize>() {
                    &mut target.as_array_mut().unwrap()[index]
                } else {
                    target.as_object_mut().unwrap().get_mut(*segment).unwrap()
                };
            }
            target.as_object_mut().unwrap().remove(path[path.len() - 1]);
            assert!(
                serde_json::from_value::<Ymm4SceneCompositionSnapshot>(json).is_err(),
                "missing required nullable field {path:?} was accepted"
            );
        }
    }

    #[tokio::test]
    async fn client_uses_current_composition_get_route_and_validates_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = serde_json::to_string(&partial_snapshot()).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /v2/scene/composition/current HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-takegraph-token: secret")
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let client = Ymm4BridgeClient::new(&format!("http://{address}"), "secret")
            .unwrap()
            .with_expected_project_id("project-a")
            .unwrap();

        let snapshot = client.current_scene_composition().await.unwrap();
        server.join().unwrap();
        assert_eq!(snapshot.frame, 120);
        assert_eq!(
            snapshot.elements[0].text.as_deref(),
            Some("現在の表示テキスト")
        );
    }

    #[test]
    fn protocol_constant_remains_v2_for_the_v2_route() {
        assert_eq!(crate::YMM4_BRIDGE_PROTOCOL_VERSION, 2);
    }
}
