//! Portable composition-graph edits from `ymm4CompositionGraphProtocol`.
//!
//! Identity, reference safety, keyframe order, effect-chain order, and sibling
//! paint order are executable here. Apply/WAL/recovery stay in the apply
//! protocols.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Quint representative identities.
pub const ROOT_GROUP_ID: EntityId = 10;
pub const MASK_SHAPE_ID: EntityId = 11;
pub const INITIAL_FOCUS_ID: EntityId = 20;
pub const STABLE_PAINT_SIBLING_ID: EntityId = 21;
pub const FIRST_EFFECT_ID: EntityId = 30;
pub const SECOND_EFFECT_ID: EntityId = 31;
pub const FRAME_BUFFER_ID: EntityId = 40;
pub const SCENE_A_ID: EntityId = 50;
pub const SCENE_B_ID: EntityId = 51;
pub const TRANSITION_ID: EntityId = 60;
/// Quint `initialRevision`.
pub const INITIAL_REVISION: i64 = 7;
const INITIAL_FINGERPRINT: i64 = 700;
const INITIAL_SOURCE_GRAPH: i64 = 100;
const INITIAL_TARGET_GRAPH: i64 = 200;
const INITIAL_DESCRIPTOR: i64 = 900;

/// Stable entity identity. Allocations are strictly monotonic.
pub type EntityId = i64;

/// Known YMM4 item families. `UnknownItem` never stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    AudioItem,
    VoiceItem,
    AssetItem,
    TextItem,
    ShapeItem,
    EffectItem,
    FrameBufferItem,
    GroupItem,
    SceneItem,
    TransitionItem,
    UnknownItem,
}

/// Keyframe interpolation modes from the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Hold,
    Linear,
    EaseIn,
    EaseOut,
}

/// Modeled composition-graph intents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionIntent {
    CreateEntity {
        kind: ItemKind,
        start_frame: i32,
        layer: i32,
        duration: i32,
    },
    UpdateGeometry {
        start_frame: i32,
        layer: i32,
        duration: i32,
    },
    UpdateKeyframes {
        keyframes: Vec<Keyframe>,
    },
    UpdatePaintOrder {
        focus_slot: usize,
    },
    AttachFocusReferences,
    DetachFocusReferences,
    AttachRootParent,
    AppendEffect {
        effect_id: EntityId,
    },
    ReorderEffects,
    DetachEffects,
    BindFrameBuffer,
    DetachFrameBuffer,
    BindScene,
    DetachScene,
    BindTransition,
    DetachTransition,
    DeleteEntity,
}

/// One keyframe: interpolation travels with its frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    pub frame: i32,
    pub interpolation: Interpolation,
}

/// Exact binding required for mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphBinding {
    pub request_digest: i64,
    pub source_graph_id: i64,
    pub target_graph_id: i64,
    pub base_revision: i64,
    pub target_fingerprint: i64,
    pub descriptor_digest: i64,
    pub target_entity_id: EntityId,
    pub target_kind: ItemKind,
}

/// One graph entity. Update never reassigns `id` / `birth_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEntity {
    pub id: EntityId,
    pub birth_id: EntityId,
    pub kind: ItemKind,
    pub start_frame: i32,
    pub layer: i32,
    pub duration: i32,
    pub parent_id: Option<EntityId>,
    pub group_id: Option<EntityId>,
    pub mask_id: Option<EntityId>,
    pub keyframes: Vec<Keyframe>,
}

/// Outcome of applying a staged intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompositionOutcome {
    Applied,
    NormalizedNoOp,
}

/// Fail-closed composition errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompositionError {
    #[error("unknown item kind cannot be staged or applied")]
    UnknownKind,
    #[error("composition graph has no staged intent")]
    NothingStaged,
    #[error("create requires an empty focus slot")]
    CreateOverExisting,
    #[error("target entity {0} does not exist")]
    MissingTarget(EntityId),
    #[error("binding does not match the current graph")]
    StaleBinding,
    #[error("geometry is illegal")]
    IllegalGeometry,
    #[error("reference would dangle or form a cycle")]
    UnsafeReference,
    #[error("entity still has incoming or outgoing references")]
    ReferencedDelete,
    #[error("paint slot {0} is out of range")]
    IllegalPaintSlot(usize),
    #[error("effect chain cannot accept this mutation")]
    IllegalEffectChain,
    #[error("intent is not valid for the staged target kind")]
    KindMismatch,
}

/// Portable composition graph plus one staged binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionGraph {
    pub source_graph_id: i64,
    pub target_graph_id: i64,
    pub target_fingerprint: i64,
    pub descriptor_digest: i64,
    pub revision: i64,
    pub semantic_mutation_count: i64,
    pub request_counter: i64,
    pub last_allocated_id: EntityId,
    pub last_deleted_focus_id: Option<EntityId>,
    pub focus_id: Option<EntityId>,
    pub entities: BTreeMap<EntityId, GraphEntity>,
    pub tombstones: BTreeSet<EntityId>,
    pub paint_order: Vec<EntityId>,
    pub effect_chain: Vec<EntityId>,
    pub effect_owners: BTreeMap<EntityId, EntityId>,
    pub frame_buffer_source: Option<EntityId>,
    pub scene_frame_buffers: BTreeMap<EntityId, EntityId>,
    pub transition_scenes: Option<(EntityId, EntityId)>,
    staged: Option<(CompositionIntent, GraphBinding)>,
}

impl CompositionGraph {
    /// Quint `init` representative: focus text item plus fixed fixtures.
    #[must_use]
    pub fn representative() -> Self {
        let mut entities = BTreeMap::new();
        entities.insert(
            ROOT_GROUP_ID,
            GraphEntity {
                id: ROOT_GROUP_ID,
                birth_id: ROOT_GROUP_ID,
                kind: ItemKind::GroupItem,
                start_frame: 0,
                layer: 0,
                duration: 1,
                parent_id: None,
                group_id: None,
                mask_id: None,
                keyframes: Vec::new(),
            },
        );
        entities.insert(
            MASK_SHAPE_ID,
            GraphEntity {
                id: MASK_SHAPE_ID,
                birth_id: MASK_SHAPE_ID,
                kind: ItemKind::ShapeItem,
                start_frame: 0,
                layer: 0,
                duration: 1,
                parent_id: None,
                group_id: None,
                mask_id: None,
                keyframes: Vec::new(),
            },
        );
        entities.insert(
            INITIAL_FOCUS_ID,
            focus_entity(INITIAL_FOCUS_ID, ItemKind::TextItem, 0, 1, 10),
        );
        entities.insert(
            STABLE_PAINT_SIBLING_ID,
            focus_entity(STABLE_PAINT_SIBLING_ID, ItemKind::TextItem, 0, 0, 10),
        );
        entities.insert(
            FIRST_EFFECT_ID,
            focus_entity(FIRST_EFFECT_ID, ItemKind::EffectItem, 0, 0, 1),
        );
        entities.insert(
            SECOND_EFFECT_ID,
            focus_entity(SECOND_EFFECT_ID, ItemKind::EffectItem, 0, 0, 1),
        );
        entities.insert(
            FRAME_BUFFER_ID,
            focus_entity(FRAME_BUFFER_ID, ItemKind::FrameBufferItem, 0, 0, 1),
        );
        entities.insert(
            SCENE_A_ID,
            focus_entity(SCENE_A_ID, ItemKind::SceneItem, 0, 0, 1),
        );
        entities.insert(
            SCENE_B_ID,
            focus_entity(SCENE_B_ID, ItemKind::SceneItem, 0, 0, 1),
        );
        entities.insert(
            TRANSITION_ID,
            focus_entity(TRANSITION_ID, ItemKind::TransitionItem, 0, 0, 1),
        );
        Self {
            source_graph_id: INITIAL_SOURCE_GRAPH,
            target_graph_id: INITIAL_TARGET_GRAPH,
            target_fingerprint: INITIAL_FINGERPRINT,
            descriptor_digest: INITIAL_DESCRIPTOR,
            revision: INITIAL_REVISION,
            semantic_mutation_count: 0,
            request_counter: 0,
            last_allocated_id: TRANSITION_ID,
            last_deleted_focus_id: None,
            focus_id: Some(INITIAL_FOCUS_ID),
            entities,
            tombstones: BTreeSet::new(),
            paint_order: vec![INITIAL_FOCUS_ID, STABLE_PAINT_SIBLING_ID],
            effect_chain: Vec::new(),
            effect_owners: BTreeMap::new(),
            frame_buffer_source: None,
            scene_frame_buffers: BTreeMap::new(),
            transition_scenes: None,
            staged: None,
        }
    }

    /// Stages an intent against `target` after capturing an exact binding.
    ///
    /// # Errors
    ///
    /// Returns [`CompositionError::UnknownKind`] for `UnknownItem`. Create over
    /// an existing focus is rejected.
    pub fn stage(
        &mut self,
        intent: CompositionIntent,
        target: EntityId,
        kind: ItemKind,
    ) -> Result<GraphBinding, CompositionError> {
        if !is_known_kind(kind) {
            return Err(CompositionError::UnknownKind);
        }
        if matches!(intent, CompositionIntent::CreateEntity { .. }) && self.focus_id.is_some() {
            return Err(CompositionError::CreateOverExisting);
        }
        self.request_counter += 1;
        let binding = GraphBinding {
            request_digest: self.request_counter,
            source_graph_id: self.source_graph_id,
            target_graph_id: self.target_graph_id,
            base_revision: self.revision,
            target_fingerprint: self.target_fingerprint,
            descriptor_digest: self.descriptor_digest,
            target_entity_id: target,
            target_kind: kind,
        };
        self.staged = Some((intent, binding));
        Ok(binding)
    }

    /// Convenience: stage a known-kind create into the empty focus slot.
    ///
    /// # Errors
    ///
    /// Returns [`CompositionError::UnknownKind`] or
    /// [`CompositionError::CreateOverExisting`].
    pub fn stage_create(&mut self, kind: ItemKind) -> Result<GraphBinding, CompositionError> {
        let next = self.last_allocated_id + 1;
        self.stage(
            CompositionIntent::CreateEntity {
                kind,
                start_frame: 0,
                layer: 0,
                duration: 10,
            },
            next,
            kind,
        )
    }

    /// Applies the staged intent if the binding still matches the live graph.
    ///
    /// # Errors
    ///
    /// Returns a modeled fail-closed error when the binding is stale or the
    /// intent is illegal.
    pub fn apply_staged(&mut self) -> Result<CompositionOutcome, CompositionError> {
        let (intent, binding) = self.staged.take().ok_or(CompositionError::NothingStaged)?;
        if !self.binding_matches(&binding) {
            self.staged = Some((intent, binding));
            return Err(CompositionError::StaleBinding);
        }
        match intent {
            CompositionIntent::CreateEntity {
                kind,
                start_frame,
                layer,
                duration,
            } => self.apply_create(binding, kind, start_frame, layer, duration),
            CompositionIntent::UpdateGeometry {
                start_frame,
                layer,
                duration,
            } => self.apply_geometry(binding, start_frame, layer, duration),
            CompositionIntent::UpdateKeyframes { keyframes } => {
                self.apply_keyframes(binding, keyframes)
            }
            CompositionIntent::UpdatePaintOrder { focus_slot } => {
                self.apply_paint_order(binding, focus_slot)
            }
            CompositionIntent::AttachFocusReferences => self.apply_attach_focus_refs(binding),
            CompositionIntent::DetachFocusReferences => self.apply_detach_focus_refs(binding),
            CompositionIntent::AttachRootParent => self.apply_attach_root_parent(binding),
            CompositionIntent::AppendEffect { effect_id } => {
                self.apply_append_effect(binding, effect_id)
            }
            CompositionIntent::ReorderEffects => self.apply_reorder_effects(binding),
            CompositionIntent::DetachEffects => self.apply_detach_effects(binding),
            CompositionIntent::BindFrameBuffer => self.apply_bind_frame_buffer(binding),
            CompositionIntent::DetachFrameBuffer => self.apply_detach_frame_buffer(binding),
            CompositionIntent::BindScene => self.apply_bind_scene(binding),
            CompositionIntent::DetachScene => self.apply_detach_scene(binding),
            CompositionIntent::BindTransition => self.apply_bind_transition(binding),
            CompositionIntent::DetachTransition => self.apply_detach_transition(binding),
            CompositionIntent::DeleteEntity => self.apply_delete(binding),
        }
    }

    /// Environment drift after staging: target graph identity changes.
    pub fn switch_target_graph(&mut self) {
        self.target_graph_id += 1;
    }

    /// Environment drift after staging: descriptor digest changes.
    pub fn change_descriptor(&mut self) {
        self.descriptor_digest += 1;
    }

    /// Current focus entity, if the slot is occupied.
    #[must_use]
    pub fn focus(&self) -> Option<&GraphEntity> {
        self.focus_id.and_then(|id| self.entities.get(&id))
    }

    fn binding_matches(&self, binding: &GraphBinding) -> bool {
        binding.request_digest == self.request_counter
            && binding.source_graph_id == self.source_graph_id
            && binding.target_graph_id == self.target_graph_id
            && binding.base_revision == self.revision
            && binding.target_fingerprint == self.target_fingerprint
            && binding.descriptor_digest == self.descriptor_digest
            && is_known_kind(binding.target_kind)
    }

    fn existing_target_is_exact(&self, binding: &GraphBinding) -> Result<(), CompositionError> {
        if !self.binding_matches(binding) {
            return Err(CompositionError::StaleBinding);
        }
        let Some(entity) = self.entities.get(&binding.target_entity_id) else {
            return Err(CompositionError::MissingTarget(binding.target_entity_id));
        };
        if entity.kind != binding.target_kind {
            return Err(CompositionError::KindMismatch);
        }
        Ok(())
    }

    fn apply_create(
        &mut self,
        binding: GraphBinding,
        kind: ItemKind,
        start_frame: i32,
        layer: i32,
        duration: i32,
    ) -> Result<CompositionOutcome, CompositionError> {
        if !self.binding_matches(&binding) || !is_known_kind(kind) {
            return Err(CompositionError::StaleBinding);
        }
        if self.focus_id.is_some() {
            return Err(CompositionError::CreateOverExisting);
        }
        if binding.target_entity_id != self.last_allocated_id + 1 {
            return Err(CompositionError::StaleBinding);
        }
        if !geometry_legal(start_frame, layer, duration, &[]) {
            return Err(CompositionError::IllegalGeometry);
        }
        let id = binding.target_entity_id;
        self.entities
            .insert(id, focus_entity(id, kind, start_frame, layer, duration));
        self.focus_id = Some(id);
        self.last_allocated_id = id;
        self.paint_order = vec![STABLE_PAINT_SIBLING_ID, id];
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_geometry(
        &mut self,
        binding: GraphBinding,
        start_frame: i32,
        layer: i32,
        duration: i32,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        let focus = self
            .focus_mut_for(&binding)
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        if !geometry_legal(start_frame, layer, duration, &focus.keyframes) {
            return Err(CompositionError::IllegalGeometry);
        }
        if focus.start_frame == start_frame && focus.layer == layer && focus.duration == duration {
            return Ok(CompositionOutcome::NormalizedNoOp);
        }
        focus.start_frame = start_frame;
        focus.layer = layer;
        focus.duration = duration;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_keyframes(
        &mut self,
        binding: GraphBinding,
        mut keyframes: Vec<Keyframe>,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        canonicalize_keyframes(&mut keyframes);
        let focus = self
            .focus_mut_for(&binding)
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        if !geometry_legal(focus.start_frame, focus.layer, focus.duration, &keyframes) {
            return Err(CompositionError::IllegalGeometry);
        }
        if focus.keyframes == keyframes {
            return Ok(CompositionOutcome::NormalizedNoOp);
        }
        focus.keyframes = keyframes;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_paint_order(
        &mut self,
        binding: GraphBinding,
        focus_slot: usize,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        let focus_id = binding.target_entity_id;
        if self.paint_order.len() != 2 || !self.paint_order.contains(&focus_id) {
            return Err(CompositionError::IllegalPaintSlot(focus_slot));
        }
        let sibling = *self
            .paint_order
            .iter()
            .find(|id| **id != focus_id)
            .ok_or(CompositionError::IllegalPaintSlot(focus_slot))?;
        let desired = match focus_slot {
            1 => vec![focus_id, sibling],
            2 => vec![sibling, focus_id],
            _ => return Err(CompositionError::IllegalPaintSlot(focus_slot)),
        };
        if self.paint_order == desired {
            return Ok(CompositionOutcome::NormalizedNoOp);
        }
        self.paint_order = desired;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_attach_focus_refs(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        let focus = self
            .focus()
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        if !is_visual_kind(focus.kind)
            || !self.entities.contains_key(&ROOT_GROUP_ID)
            || !self.entities.contains_key(&MASK_SHAPE_ID)
        {
            return Err(CompositionError::UnsafeReference);
        }
        if self.root_group_parent() == Some(focus.id) {
            return Err(CompositionError::UnsafeReference);
        }
        let focus = self
            .focus_mut_for(&binding)
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        focus.parent_id = Some(ROOT_GROUP_ID);
        focus.group_id = Some(ROOT_GROUP_ID);
        focus.mask_id = Some(MASK_SHAPE_ID);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_detach_focus_refs(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        let focus = self
            .focus_mut_for(&binding)
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        focus.parent_id = None;
        focus.group_id = None;
        focus.mask_id = None;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_attach_root_parent(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if binding.target_entity_id != ROOT_GROUP_ID {
            return Err(CompositionError::KindMismatch);
        }
        let focus = self
            .focus()
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        if focus.kind != ItemKind::GroupItem {
            return Err(CompositionError::KindMismatch);
        }
        if focus.parent_id == Some(ROOT_GROUP_ID) || focus.group_id == Some(ROOT_GROUP_ID) {
            return Err(CompositionError::UnsafeReference);
        }
        if self.root_group_parent().is_some() {
            return Err(CompositionError::UnsafeReference);
        }
        let focus_id = focus.id;
        if let Some(root) = self.entities.get_mut(&ROOT_GROUP_ID) {
            root.parent_id = Some(focus_id);
        }
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_append_effect(
        &mut self,
        binding: GraphBinding,
        effect_id: EntityId,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if !self.entities.contains_key(&effect_id) {
            return Err(CompositionError::MissingTarget(effect_id));
        }
        if self.effect_chain.contains(&effect_id) || self.effect_chain.len() >= 2 {
            return Err(CompositionError::IllegalEffectChain);
        }
        if self.effect_chain.is_empty() && effect_id != FIRST_EFFECT_ID {
            return Err(CompositionError::IllegalEffectChain);
        }
        if self.effect_chain.len() == 1 && effect_id != SECOND_EFFECT_ID {
            return Err(CompositionError::IllegalEffectChain);
        }
        self.effect_chain.push(effect_id);
        self.effect_owners
            .insert(effect_id, binding.target_entity_id);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_reorder_effects(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if self.effect_chain.as_slice() != [FIRST_EFFECT_ID, SECOND_EFFECT_ID] {
            return Err(CompositionError::IllegalEffectChain);
        }
        self.effect_chain = vec![SECOND_EFFECT_ID, FIRST_EFFECT_ID];
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_detach_effects(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        self.effect_chain.clear();
        self.effect_owners.clear();
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_bind_frame_buffer(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        let focus = self
            .focus()
            .ok_or(CompositionError::MissingTarget(binding.target_entity_id))?;
        if binding.target_entity_id != FRAME_BUFFER_ID
            || !is_visual_kind(focus.kind)
            || self.frame_buffer_source.is_some()
        {
            return Err(CompositionError::UnsafeReference);
        }
        self.frame_buffer_source = Some(focus.id);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_detach_frame_buffer(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        self.frame_buffer_source = None;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_bind_scene(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if binding.target_entity_id != SCENE_A_ID || !self.entities.contains_key(&FRAME_BUFFER_ID) {
            return Err(CompositionError::UnsafeReference);
        }
        self.scene_frame_buffers.insert(SCENE_A_ID, FRAME_BUFFER_ID);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_detach_scene(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if binding.target_entity_id != SCENE_A_ID {
            return Err(CompositionError::KindMismatch);
        }
        self.scene_frame_buffers.remove(&SCENE_A_ID);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_bind_transition(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if binding.target_entity_id != TRANSITION_ID
            || !self.entities.contains_key(&SCENE_A_ID)
            || !self.entities.contains_key(&SCENE_B_ID)
            || SCENE_A_ID == SCENE_B_ID
        {
            return Err(CompositionError::UnsafeReference);
        }
        self.transition_scenes = Some((SCENE_A_ID, SCENE_B_ID));
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_detach_transition(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if binding.target_entity_id != TRANSITION_ID {
            return Err(CompositionError::KindMismatch);
        }
        self.transition_scenes = None;
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn apply_delete(
        &mut self,
        binding: GraphBinding,
    ) -> Result<CompositionOutcome, CompositionError> {
        self.existing_target_is_exact(&binding)?;
        if !self.is_reference_safe_to_delete(binding.target_entity_id) {
            return Err(CompositionError::ReferencedDelete);
        }
        let id = binding.target_entity_id;
        self.entities.remove(&id);
        self.tombstones.insert(id);
        if self.focus_id == Some(id) {
            self.last_deleted_focus_id = Some(id);
            self.focus_id = None;
        }
        self.paint_order.retain(|slot| *slot != id);
        self.mark_applied();
        Ok(CompositionOutcome::Applied)
    }

    fn is_reference_safe_to_delete(&self, id: EntityId) -> bool {
        self.entities.contains_key(&id)
            && !self.has_incoming_references(id)
            && !self.has_outgoing_references(id)
    }

    fn has_incoming_references(&self, id: EntityId) -> bool {
        self.entities.iter().any(|(other, entity)| {
            *other != id
                && (entity.parent_id == Some(id)
                    || entity.group_id == Some(id)
                    || entity.mask_id == Some(id))
        }) || self.effect_chain.contains(&id)
            || self.effect_owners.values().any(|owner| *owner == id)
            || self.frame_buffer_source == Some(id)
            || self
                .scene_frame_buffers
                .values()
                .any(|buffer| *buffer == id)
            || self
                .transition_scenes
                .is_some_and(|(source, target)| source == id || target == id)
    }

    fn has_outgoing_references(&self, id: EntityId) -> bool {
        let Some(entity) = self.entities.get(&id) else {
            return false;
        };
        entity.parent_id.is_some()
            || entity.group_id.is_some()
            || entity.mask_id.is_some()
            || self.effect_owners.keys().any(|effect| *effect == id)
            || (id == FRAME_BUFFER_ID && self.frame_buffer_source.is_some())
            || self.scene_frame_buffers.contains_key(&id)
            || (id == TRANSITION_ID && self.transition_scenes.is_some())
    }

    fn root_group_parent(&self) -> Option<EntityId> {
        self.entities
            .get(&ROOT_GROUP_ID)
            .and_then(|root| root.parent_id)
    }

    fn focus_mut_for(&mut self, binding: &GraphBinding) -> Option<&mut GraphEntity> {
        let id = self
            .focus_id
            .filter(|focus| *focus == binding.target_entity_id)?;
        self.entities.get_mut(&id)
    }

    fn mark_applied(&mut self) {
        self.revision += 1;
        self.target_fingerprint += 1;
        self.semantic_mutation_count += 1;
    }
}

fn focus_entity(
    id: EntityId,
    kind: ItemKind,
    start_frame: i32,
    layer: i32,
    duration: i32,
) -> GraphEntity {
    GraphEntity {
        id,
        birth_id: id,
        kind,
        start_frame,
        layer,
        duration,
        parent_id: None,
        group_id: None,
        mask_id: None,
        keyframes: Vec::new(),
    }
}

/// Returns whether `kind` is a known, writable item family.
#[must_use]
pub fn is_known_kind(kind: ItemKind) -> bool {
    kind != ItemKind::UnknownItem
}

/// Returns whether `kind` may carry visual parent/group/mask references.
#[must_use]
pub fn is_visual_kind(kind: ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::AssetItem
            | ItemKind::TextItem
            | ItemKind::ShapeItem
            | ItemKind::EffectItem
            | ItemKind::FrameBufferItem
            | ItemKind::GroupItem
            | ItemKind::SceneItem
            | ItemKind::TransitionItem
    )
}

fn canonicalize_keyframes(keyframes: &mut [Keyframe]) {
    keyframes.sort_by_key(|keyframe| keyframe.frame);
}

fn geometry_legal(start_frame: i32, layer: i32, duration: i32, keyframes: &[Keyframe]) -> bool {
    start_frame >= 0
        && layer >= 0
        && duration > 0
        && keyframes
            .windows(2)
            .all(|pair| pair[0].frame < pair[1].frame)
        && keyframes.iter().all(|keyframe| keyframe.frame < duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_ok(graph: &mut CompositionGraph) -> CompositionOutcome {
        graph.apply_staged().expect("apply")
    }

    #[test]
    fn geometry_and_canonical_keyframes_keep_identity() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::UpdateGeometry {
                    start_frame: 2,
                    layer: 2,
                    duration: 12,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage geometry");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::Applied);
        graph
            .stage(
                CompositionIntent::UpdateKeyframes {
                    keyframes: vec![
                        Keyframe {
                            frame: 6,
                            interpolation: Interpolation::Linear,
                        },
                        Keyframe {
                            frame: 2,
                            interpolation: Interpolation::EaseIn,
                        },
                    ],
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage keyframes");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::Applied);
        let focus = graph.focus().expect("focus");
        assert_eq!(focus.id, INITIAL_FOCUS_ID);
        assert_eq!(focus.birth_id, INITIAL_FOCUS_ID);
        assert_eq!(focus.start_frame, 2);
        assert_eq!(focus.layer, 2);
        assert_eq!(focus.duration, 12);
        assert_eq!(
            focus.keyframes,
            [
                Keyframe {
                    frame: 2,
                    interpolation: Interpolation::EaseIn,
                },
                Keyframe {
                    frame: 6,
                    interpolation: Interpolation::Linear,
                },
            ]
        );
        assert_eq!(graph.revision, INITIAL_REVISION + 2);
    }

    #[test]
    fn geometry_and_keyframe_noops_do_not_advance_revision() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::UpdateGeometry {
                    start_frame: 0,
                    layer: 1,
                    duration: 10,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::NormalizedNoOp);
        assert_eq!(graph.revision, INITIAL_REVISION);

        graph
            .stage(
                CompositionIntent::UpdateKeyframes {
                    keyframes: vec![
                        Keyframe {
                            frame: 6,
                            interpolation: Interpolation::Linear,
                        },
                        Keyframe {
                            frame: 2,
                            interpolation: Interpolation::EaseIn,
                        },
                    ],
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage kf");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::Applied);
        graph
            .stage(
                CompositionIntent::UpdateKeyframes {
                    keyframes: vec![
                        Keyframe {
                            frame: 6,
                            interpolation: Interpolation::Linear,
                        },
                        Keyframe {
                            frame: 2,
                            interpolation: Interpolation::EaseIn,
                        },
                    ],
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage kf again");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::NormalizedNoOp);
        assert_eq!(graph.revision, INITIAL_REVISION + 1);
    }

    #[test]
    fn paint_order_is_independent_of_layer_and_effect_chain() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::UpdatePaintOrder { focus_slot: 2 },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("front");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::Applied);
        graph
            .stage(
                CompositionIntent::UpdatePaintOrder { focus_slot: 2 },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("front again");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::NormalizedNoOp);
        graph
            .stage(
                CompositionIntent::UpdatePaintOrder { focus_slot: 1 },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("back");
        assert_eq!(apply_ok(&mut graph), CompositionOutcome::Applied);
        assert_eq!(
            graph.paint_order,
            [INITIAL_FOCUS_ID, STABLE_PAINT_SIBLING_ID]
        );
        assert_eq!(graph.focus().expect("focus").layer, 1);
        assert!(graph.effect_chain.is_empty());
        assert_eq!(graph.revision, INITIAL_REVISION + 2);
    }

    #[test]
    fn unknown_kind_and_stale_binding_fail_closed() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("delete");
        apply_ok(&mut graph);
        let err = graph
            .stage_create(ItemKind::UnknownItem)
            .expect_err("unknown");
        assert_eq!(err, CompositionError::UnknownKind);
        assert!(graph.focus_id.is_none());
        assert_eq!(graph.last_allocated_id, TRANSITION_ID);

        let mut stale = CompositionGraph::representative();
        stale
            .stage(
                CompositionIntent::UpdateGeometry {
                    start_frame: 2,
                    layer: 2,
                    duration: 12,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage");
        stale.switch_target_graph();
        assert_eq!(
            stale.apply_staged().expect_err("stale graph"),
            CompositionError::StaleBinding
        );
        stale.change_descriptor();
        assert_eq!(
            stale.apply_staged().expect_err("stale descriptor"),
            CompositionError::StaleBinding
        );
        assert_eq!(stale.focus().expect("focus").start_frame, 0);
        assert_eq!(stale.revision, INITIAL_REVISION);
    }

    #[test]
    fn every_known_kind_can_be_created_after_delete() {
        let kinds = [
            ItemKind::AudioItem,
            ItemKind::VoiceItem,
            ItemKind::AssetItem,
            ItemKind::TextItem,
            ItemKind::ShapeItem,
            ItemKind::EffectItem,
            ItemKind::FrameBufferItem,
            ItemKind::GroupItem,
            ItemKind::SceneItem,
            ItemKind::TransitionItem,
        ];
        for kind in kinds {
            let mut graph = CompositionGraph::representative();
            graph
                .stage(
                    CompositionIntent::DeleteEntity,
                    INITIAL_FOCUS_ID,
                    ItemKind::TextItem,
                )
                .expect("delete");
            apply_ok(&mut graph);
            graph.stage_create(kind).expect("stage known");
            apply_ok(&mut graph);
            assert_eq!(graph.focus().expect("focus").kind, kind);
            assert_eq!(graph.focus().expect("focus").id, TRANSITION_ID + 1);
        }
    }

    #[test]
    fn create_allocates_above_high_water_and_delete_tombstones() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("delete");
        apply_ok(&mut graph);
        graph.stage_create(ItemKind::ShapeItem).expect("create");
        apply_ok(&mut graph);
        let focus = graph.focus().expect("new focus");
        assert_eq!(graph.last_deleted_focus_id, Some(INITIAL_FOCUS_ID));
        assert!(graph.tombstones.contains(&INITIAL_FOCUS_ID));
        assert_eq!(focus.kind, ItemKind::ShapeItem);
        assert_eq!(focus.id, TRANSITION_ID + 1);
        assert_eq!(focus.birth_id, focus.id);
        assert_eq!(graph.last_allocated_id, focus.id);
        assert_eq!(graph.revision, INITIAL_REVISION + 2);
    }

    #[test]
    fn referenced_delete_requires_explicit_detach() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::BindFrameBuffer,
                FRAME_BUFFER_ID,
                ItemKind::FrameBufferItem,
            )
            .expect("bind");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("delete");
        assert_eq!(
            graph.apply_staged().expect_err("referenced"),
            CompositionError::ReferencedDelete
        );
        assert!(graph.focus_id.is_some());
        graph
            .stage(
                CompositionIntent::DetachFrameBuffer,
                FRAME_BUFFER_ID,
                ItemKind::FrameBufferItem,
            )
            .expect("detach");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("delete after detach");
        apply_ok(&mut graph);
        assert!(graph.focus_id.is_none());
        assert!(graph.frame_buffer_source.is_none());
        assert_eq!(graph.revision, INITIAL_REVISION + 3);
    }

    fn assert_referenced_delete_keeps(graph: &mut CompositionGraph, id: EntityId, kind: ItemKind) {
        graph
            .stage(CompositionIntent::DeleteEntity, id, kind)
            .expect("stage referenced delete");
        assert_eq!(
            graph.apply_staged().expect_err("must stay referenced"),
            CompositionError::ReferencedDelete
        );
        assert!(graph.entities.contains_key(&id));
    }

    #[test]
    fn chained_effect_delete_fails_until_detach() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::AppendEffect {
                    effect_id: FIRST_EFFECT_ID,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("append");
        apply_ok(&mut graph);
        let chain = graph.effect_chain.clone();
        assert_referenced_delete_keeps(&mut graph, FIRST_EFFECT_ID, ItemKind::EffectItem);
        assert_eq!(graph.effect_chain, chain);
        graph
            .stage(
                CompositionIntent::DetachEffects,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("detach");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                FIRST_EFFECT_ID,
                ItemKind::EffectItem,
            )
            .expect("delete after detach");
        apply_ok(&mut graph);
        assert!(!graph.entities.contains_key(&FIRST_EFFECT_ID));
        assert!(graph.tombstones.contains(&FIRST_EFFECT_ID));
        assert!(graph.effect_chain.is_empty());
    }

    #[test]
    fn scene_bound_framebuffer_delete_fails_until_detach() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::BindScene,
                SCENE_A_ID,
                ItemKind::SceneItem,
            )
            .expect("bind");
        apply_ok(&mut graph);
        let buffers = graph.scene_frame_buffers.clone();
        assert_referenced_delete_keeps(&mut graph, FRAME_BUFFER_ID, ItemKind::FrameBufferItem);
        assert_eq!(graph.scene_frame_buffers, buffers);
        graph
            .stage(
                CompositionIntent::DetachScene,
                SCENE_A_ID,
                ItemKind::SceneItem,
            )
            .expect("detach");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                FRAME_BUFFER_ID,
                ItemKind::FrameBufferItem,
            )
            .expect("delete after detach");
        apply_ok(&mut graph);
        assert!(!graph.entities.contains_key(&FRAME_BUFFER_ID));
        assert!(graph.scene_frame_buffers.is_empty());
    }

    #[test]
    fn parent_and_mask_delete_fails_until_focus_refs_detach() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::AttachFocusReferences,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("attach");
        apply_ok(&mut graph);
        assert_referenced_delete_keeps(&mut graph, ROOT_GROUP_ID, ItemKind::GroupItem);
        assert_eq!(
            graph.entities[&INITIAL_FOCUS_ID].parent_id,
            Some(ROOT_GROUP_ID)
        );
        assert_referenced_delete_keeps(&mut graph, MASK_SHAPE_ID, ItemKind::ShapeItem);
        assert_eq!(
            graph.entities[&INITIAL_FOCUS_ID].mask_id,
            Some(MASK_SHAPE_ID)
        );
        graph
            .stage(
                CompositionIntent::DetachFocusReferences,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("detach");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                ROOT_GROUP_ID,
                ItemKind::GroupItem,
            )
            .expect("delete root");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                MASK_SHAPE_ID,
                ItemKind::ShapeItem,
            )
            .expect("delete mask");
        apply_ok(&mut graph);
        assert!(!graph.entities.contains_key(&ROOT_GROUP_ID));
        assert!(!graph.entities.contains_key(&MASK_SHAPE_ID));
    }

    #[test]
    fn transition_endpoint_scene_delete_fails_until_detach() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::BindTransition,
                TRANSITION_ID,
                ItemKind::TransitionItem,
            )
            .expect("bind");
        apply_ok(&mut graph);
        assert_referenced_delete_keeps(&mut graph, SCENE_A_ID, ItemKind::SceneItem);
        assert_eq!(graph.transition_scenes, Some((SCENE_A_ID, SCENE_B_ID)));
        graph
            .stage(
                CompositionIntent::DetachTransition,
                TRANSITION_ID,
                ItemKind::TransitionItem,
            )
            .expect("detach");
        apply_ok(&mut graph);
        graph
            .stage(
                CompositionIntent::DeleteEntity,
                SCENE_A_ID,
                ItemKind::SceneItem,
            )
            .expect("delete after detach");
        apply_ok(&mut graph);
        assert!(!graph.entities.contains_key(&SCENE_A_ID));
        assert!(graph.transition_scenes.is_none());
    }

    #[test]
    fn parent_cycle_and_effect_reorder_and_render_chain() {
        let mut cycle = CompositionGraph::representative();
        cycle
            .stage(
                CompositionIntent::DeleteEntity,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("delete");
        apply_ok(&mut cycle);
        cycle.stage_create(ItemKind::GroupItem).expect("group");
        apply_ok(&mut cycle);
        let group_id = cycle.focus_id.expect("group");
        cycle
            .stage(
                CompositionIntent::AttachFocusReferences,
                group_id,
                ItemKind::GroupItem,
            )
            .expect("attach");
        apply_ok(&mut cycle);
        cycle
            .stage(
                CompositionIntent::AttachRootParent,
                ROOT_GROUP_ID,
                ItemKind::GroupItem,
            )
            .expect("cycle");
        assert_eq!(
            cycle.apply_staged().expect_err("cycle"),
            CompositionError::UnsafeReference
        );
        assert!(cycle.entities[&ROOT_GROUP_ID].parent_id.is_none());

        let mut effects = CompositionGraph::representative();
        effects
            .stage(
                CompositionIntent::AppendEffect {
                    effect_id: FIRST_EFFECT_ID,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("first");
        apply_ok(&mut effects);
        effects
            .stage(
                CompositionIntent::AppendEffect {
                    effect_id: SECOND_EFFECT_ID,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("second");
        apply_ok(&mut effects);
        effects
            .stage(
                CompositionIntent::ReorderEffects,
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("reorder");
        apply_ok(&mut effects);
        assert_eq!(effects.effect_chain, [SECOND_EFFECT_ID, FIRST_EFFECT_ID]);
        assert_eq!(effects.effect_owners[&FIRST_EFFECT_ID], INITIAL_FOCUS_ID);

        let mut render = CompositionGraph::representative();
        render
            .stage(
                CompositionIntent::BindFrameBuffer,
                FRAME_BUFFER_ID,
                ItemKind::FrameBufferItem,
            )
            .expect("fb");
        apply_ok(&mut render);
        render
            .stage(
                CompositionIntent::BindScene,
                SCENE_A_ID,
                ItemKind::SceneItem,
            )
            .expect("scene");
        apply_ok(&mut render);
        render
            .stage(
                CompositionIntent::BindTransition,
                TRANSITION_ID,
                ItemKind::TransitionItem,
            )
            .expect("transition");
        apply_ok(&mut render);
        assert_eq!(render.frame_buffer_source, Some(INITIAL_FOCUS_ID));
        assert_eq!(render.scene_frame_buffers[&SCENE_A_ID], FRAME_BUFFER_ID);
        assert_eq!(render.transition_scenes, Some((SCENE_A_ID, SCENE_B_ID)));
    }

    #[test]
    fn illegal_geometry_is_rejected_from_staged_state() {
        let mut graph = CompositionGraph::representative();
        graph
            .stage(
                CompositionIntent::UpdateGeometry {
                    start_frame: 0,
                    layer: 1,
                    duration: 0,
                },
                INITIAL_FOCUS_ID,
                ItemKind::TextItem,
            )
            .expect("stage");
        assert_eq!(
            graph.apply_staged().expect_err("illegal"),
            CompositionError::IllegalGeometry
        );
        assert_eq!(graph.revision, INITIAL_REVISION);
    }
}
