//! Portable project-edit families from `ymm4ProjectEditProtocol`.
//!
//! Every broad family starts unavailable. Staging fail-closes unless the
//! matching versioned capability is advertised. `NativeTemplateInstantiate`
//! remains the already-modeled template path.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Quint `initialHeadRevision`.
pub const INITIAL_HEAD_REVISION: i64 = 20;

/// Project-edit lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEditStatus {
    ProjectIdle,
    ProjectPreviewable,
    ProjectApproved,
    ProjectConflicted,
    ProjectRejected,
    ProjectCommitted,
    ProjectNoOp,
}

/// Modeled project-edit families. Definition edits stay unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEditKind {
    NoProjectEdit,
    ProjectSettingsEdit,
    SceneCreate,
    SceneUpdate,
    SceneDelete,
    TimelineCreate,
    TimelineUpdate,
    TimelineDelete,
    CharacterCreate,
    CharacterUpdate,
    CharacterDelete,
    NativeTemplateInstantiate,
    NativeTemplateDefinitionEdit,
}

/// Sealed project-setting taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSettingClass {
    NoProjectSetting,
    CanvasSize,
    FrameRate,
    AudioSampleRate,
    BackgroundColor,
    TimelineLayerPolicy,
}

/// A character update is exactly one known setting class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterSettingClass {
    NoCharacterSetting,
    CharacterName,
    CharacterGroup,
    CharacterVoiceType,
    CharacterVoiceParameters,
    CharacterPortraitConfiguration,
}

/// How dependents are treated for delete/update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependentPolicy {
    NoDependentPolicy,
    RejectWhenReferenced,
    RewriteManagedDependents,
}

/// Versioned capability advertisement for one project-edit family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectEditCapability {
    SettingsMutation,
    SceneMutation,
    TimelineMutation,
    CharacterMutation,
    TemplateMutation,
}

/// Approval-bound project-edit plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectEditPlan {
    pub kind: ProjectEditKind,
    pub setting_class: ProjectSettingClass,
    pub character_setting_class: CharacterSettingClass,
    pub character_name_preimage_digest: Option<i64>,
    pub character_group_preimage_digest: Option<i64>,
    pub character_voice_type_preimage_digest: Option<i64>,
    pub character_voice_parameters_preimage_digest: Option<i64>,
    pub character_portrait_configuration_preimage_digest: Option<i64>,
    pub unknown_character_configuration_preimage_digest: Option<i64>,
    pub digest: i64,
    pub base_revision: i64,
    pub source_fingerprint: i64,
    pub edit_surface_revision: i64,
    pub capability_revision: i64,
    pub target_resource_revision: i64,
    pub descriptor_catalog_digest: i64,
    pub dependent_set_digest: i64,
    pub dependent_count: i64,
    pub dependent_policy: DependentPolicy,
    pub template_content_digest: i64,
    pub expansion_digest: i64,
    pub predicted_changed_entities: i64,
    pub normalized_no_op: bool,
}

/// Executable project-edit session. Families start unavailable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct ProjectEditSession {
    pub status: ProjectEditStatus,
    pub head_revision: i64,
    pub source_fingerprint: i64,
    pub edit_surface_revision: i64,
    pub capability_revision: i64,
    pub descriptor_catalog_revision: i64,
    pub settings_mutation_supported: bool,
    pub scene_mutation_supported: bool,
    pub timeline_mutation_supported: bool,
    pub character_mutation_supported: bool,
    pub template_mutation_supported: bool,
    pub settings_revision: i64,
    pub scene_topology_revision: i64,
    pub character_catalog_revision: i64,
    pub template_catalog_revision: i64,
    pub scene_count: i64,
    pub referenced_scene_count: i64,
    pub scene_reference_set_digest: i64,
    pub timeline_count: i64,
    pub referenced_timeline_count: i64,
    pub timeline_reference_set_digest: i64,
    pub character_count: i64,
    pub character_dependent_count: i64,
    pub managed_character_dependent_count: i64,
    pub dependent_set_digest: i64,
    pub character_name_digest: i64,
    pub character_group_digest: i64,
    pub character_voice_type_digest: i64,
    pub character_voice_parameters_digest: i64,
    pub character_portrait_configuration_digest: i64,
    pub unknown_character_configuration_digest: i64,
    pub template_descriptor_available: bool,
    pub template_content_digest: i64,
    pub template_expansion_digest: i64,
    pub template_produced_kinds_supported: bool,
    pub instantiated_template_items: i64,
    pub derived_epoch: i64,
    pub checkpoint_epoch: i64,
    pub inspection_epoch: i64,
    pub render_epoch: i64,
    pub composition_epoch: i64,
    pub timeline_plan_epoch: i64,
    pub project_mutation_count: i64,
    pub canonical_commit_count: i64,
    pub observation_count: i64,
    pub last_rejected_kind: ProjectEditKind,
    pub plan: Option<ProjectEditPlan>,
    pub approved_plan: Option<ProjectEditPlan>,
    pub committed_plan: Option<ProjectEditPlan>,
}

/// Fail-closed project-edit errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectEditError {
    #[error("project-edit family {0:?} is not advertised")]
    UnsupportedFamily(ProjectEditKind),
    #[error("native template definition edits are unavailable")]
    TemplateDefinitionUnavailable,
    #[error("delete is referenced and reject-when-referenced is in force")]
    ReferencedDelete,
    #[error("character dependents include unmanaged unknowns")]
    UnknownCharacterDependents,
    #[error("project edit is not in the expected status")]
    UnexpectedStatus,
    #[error("plan binding is stale")]
    StalePlan,
    #[error("character update must name exactly one known setting class")]
    InvalidCharacterSetting,
    #[error("template content/expansion binding is missing")]
    UnboundTemplate,
}

impl ProjectEditSession {
    /// Protocol `init`: every mutation family unavailable.
    #[must_use]
    pub fn initial() -> Self {
        Self {
            status: ProjectEditStatus::ProjectIdle,
            head_revision: INITIAL_HEAD_REVISION,
            source_fingerprint: 100,
            edit_surface_revision: 1,
            capability_revision: 1,
            descriptor_catalog_revision: 1,
            settings_mutation_supported: false,
            scene_mutation_supported: false,
            timeline_mutation_supported: false,
            character_mutation_supported: false,
            template_mutation_supported: false,
            settings_revision: 5,
            scene_topology_revision: 6,
            character_catalog_revision: 7,
            template_catalog_revision: 8,
            scene_count: 2,
            referenced_scene_count: 1,
            scene_reference_set_digest: 60,
            timeline_count: 2,
            referenced_timeline_count: 1,
            timeline_reference_set_digest: 70,
            character_count: 2,
            character_dependent_count: 2,
            managed_character_dependent_count: 2,
            dependent_set_digest: 90,
            character_name_digest: 101,
            character_group_digest: 102,
            character_voice_type_digest: 103,
            character_voice_parameters_digest: 104,
            character_portrait_configuration_digest: 105,
            unknown_character_configuration_digest: 199,
            template_descriptor_available: false,
            template_content_digest: 0,
            template_expansion_digest: 0,
            template_produced_kinds_supported: false,
            instantiated_template_items: 0,
            derived_epoch: 0,
            checkpoint_epoch: 0,
            inspection_epoch: 0,
            render_epoch: 0,
            composition_epoch: 0,
            timeline_plan_epoch: 0,
            project_mutation_count: 0,
            canonical_commit_count: 0,
            observation_count: 0,
            last_rejected_kind: ProjectEditKind::NoProjectEdit,
            plan: None,
            approved_plan: None,
            committed_plan: None,
        }
    }

    /// Advertises one family and advances the edit-surface binding.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectEditError::UnexpectedStatus`] unless Idle.
    pub fn enable(&mut self, capability: ProjectEditCapability) -> Result<(), ProjectEditError> {
        self.require_idle()?;
        match capability {
            ProjectEditCapability::SettingsMutation => self.settings_mutation_supported = true,
            ProjectEditCapability::SceneMutation => self.scene_mutation_supported = true,
            ProjectEditCapability::TimelineMutation => self.timeline_mutation_supported = true,
            ProjectEditCapability::CharacterMutation => self.character_mutation_supported = true,
            ProjectEditCapability::TemplateMutation => {
                self.template_mutation_supported = true;
                self.template_descriptor_available = true;
                self.template_produced_kinds_supported = true;
                self.template_catalog_revision += 1;
                self.template_content_digest = 81;
                self.template_expansion_digest = 82;
            }
        }
        self.edit_surface_revision += 1;
        self.capability_revision += 1;
        self.descriptor_catalog_revision += 1;
        Ok(())
    }

    /// Records a read-only observation. Does not mutate the project.
    pub fn observe(&mut self) {
        if self.status == ProjectEditStatus::ProjectIdle {
            self.observation_count += 1;
        }
    }

    /// Clears referenced-scene evidence so an unreferenced delete can be staged.
    pub fn select_unreferenced_scene_target(&mut self) {
        if self.status == ProjectEditStatus::ProjectIdle && self.referenced_scene_count > 0 {
            self.referenced_scene_count = 0;
            self.scene_reference_set_digest += 1;
            self.observation_count += 1;
        }
    }

    /// Clears referenced-timeline evidence so an unreferenced delete can be staged.
    pub fn select_unreferenced_timeline_target(&mut self) {
        if self.status == ProjectEditStatus::ProjectIdle && self.referenced_timeline_count > 0 {
            self.referenced_timeline_count = 0;
            self.timeline_reference_set_digest += 1;
            self.observation_count += 1;
        }
    }

    /// Adds one unmanaged character dependent for fail-closed update/delete tests.
    pub fn select_character_with_unknown_dependent(&mut self) {
        if self.status == ProjectEditStatus::ProjectIdle
            && self.character_dependent_count == self.managed_character_dependent_count
        {
            self.character_dependent_count += 1;
            self.dependent_set_digest += 1;
            self.observation_count += 1;
        }
    }

    /// Clears character dependents.
    pub fn select_character_without_dependents(&mut self) {
        if self.status == ProjectEditStatus::ProjectIdle && self.character_dependent_count > 0 {
            self.character_dependent_count = 0;
            self.managed_character_dependent_count = 0;
            self.dependent_set_digest += 1;
            self.observation_count += 1;
        }
    }

    /// Stages a family or fail-closes when it is unadvertised / illegal.
    ///
    /// # Errors
    ///
    /// Returns a modeled reject without sealing a plan.
    pub fn stage(
        &mut self,
        kind: ProjectEditKind,
        setting_class: ProjectSettingClass,
        character_setting_class: CharacterSettingClass,
        policy: DependentPolicy,
        changed_entities: i64,
        normalized_no_op: bool,
    ) -> Result<ProjectEditPlan, ProjectEditError> {
        self.require_idle()?;
        if kind == ProjectEditKind::NativeTemplateDefinitionEdit {
            return self.reject(kind, ProjectEditError::TemplateDefinitionUnavailable);
        }
        if !self.kind_is_supported(kind) {
            return self.reject(kind, ProjectEditError::UnsupportedFamily(kind));
        }
        if matches!(
            kind,
            ProjectEditKind::SceneDelete | ProjectEditKind::TimelineDelete
        ) && policy == DependentPolicy::RejectWhenReferenced
            && self.dependent_count_for(kind) > 0
        {
            return self.reject(kind, ProjectEditError::ReferencedDelete);
        }
        if matches!(
            kind,
            ProjectEditKind::CharacterUpdate | ProjectEditKind::CharacterDelete
        ) && self.character_dependent_count > self.managed_character_dependent_count
        {
            return self.reject(kind, ProjectEditError::UnknownCharacterDependents);
        }
        if kind == ProjectEditKind::CharacterUpdate
            && character_setting_class == CharacterSettingClass::NoCharacterSetting
        {
            return self.reject(kind, ProjectEditError::InvalidCharacterSetting);
        }
        if kind == ProjectEditKind::NativeTemplateInstantiate
            && (!self.template_descriptor_available
                || !self.template_produced_kinds_supported
                || self.template_content_digest == 0
                || self.template_expansion_digest == 0)
        {
            return self.reject(kind, ProjectEditError::UnboundTemplate);
        }
        let plan = self.seal_plan(
            kind,
            setting_class,
            character_setting_class,
            policy,
            changed_entities,
            normalized_no_op,
        );
        self.status = ProjectEditStatus::ProjectPreviewable;
        self.plan = Some(plan.clone());
        Ok(plan)
    }

    /// Approves the exact staged plan while it remains fresh.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectEditError::StalePlan`] when the binding drifted.
    pub fn approve(&mut self) -> Result<(), ProjectEditError> {
        if self.status != ProjectEditStatus::ProjectPreviewable {
            return Err(ProjectEditError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .clone()
            .ok_or(ProjectEditError::UnexpectedStatus)?;
        if plan.kind == ProjectEditKind::NoProjectEdit
            || plan.digest == 0
            || !self.kind_is_supported(plan.kind)
            || !self.character_plan_preserves_dependents(&plan)
            || !self.plan_is_fresh(&plan)
        {
            return Err(ProjectEditError::StalePlan);
        }
        self.status = ProjectEditStatus::ProjectApproved;
        self.approved_plan = Some(plan);
        Ok(())
    }

    /// Advances the source revision/fingerprint after approval.
    pub fn drift_source(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.head_revision += 1;
            self.source_fingerprint += 1;
        }
    }

    /// Advances the edit-surface and capability revisions after approval.
    pub fn drift_surface(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.edit_surface_revision += 1;
            self.capability_revision += 1;
        }
    }

    /// Advances only the capability revision after approval.
    pub fn drift_capability(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.capability_revision += 1;
        }
    }

    /// Advances the descriptor catalog revision after approval.
    pub fn drift_descriptor(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.descriptor_catalog_revision += 1;
        }
    }

    /// Adds one managed character dependent after approval.
    pub fn drift_character_dependents(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.character_dependent_count += 1;
            self.managed_character_dependent_count += 1;
            self.dependent_set_digest += 1;
            self.observation_count += 1;
        }
    }

    /// Advances template catalog/content/expansion digests after approval.
    pub fn drift_template(&mut self) {
        if self.status == ProjectEditStatus::ProjectApproved {
            self.template_catalog_revision += 1;
            self.template_content_digest += 1;
            self.template_expansion_digest += 1;
        }
    }

    /// Marks an approved plan conflicted when it is no longer fresh.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectEditError::UnexpectedStatus`] when the plan is still fresh.
    pub fn mark_conflict(&mut self) -> Result<(), ProjectEditError> {
        if self.status != ProjectEditStatus::ProjectApproved {
            return Err(ProjectEditError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .as_ref()
            .ok_or(ProjectEditError::UnexpectedStatus)?;
        if self.plan_is_fresh(plan) {
            return Err(ProjectEditError::UnexpectedStatus);
        }
        self.status = ProjectEditStatus::ProjectConflicted;
        Ok(())
    }

    /// Commits an actual edit or finishes a normalized no-op without a revision.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectEditError::StalePlan`] when approval is not exact/fresh.
    pub fn commit(&mut self) -> Result<ProjectEditStatus, ProjectEditError> {
        if self.status != ProjectEditStatus::ProjectApproved {
            return Err(ProjectEditError::UnexpectedStatus);
        }
        let plan = self
            .plan
            .clone()
            .ok_or(ProjectEditError::UnexpectedStatus)?;
        if self.approved_plan.as_ref() != Some(&plan)
            || !self.kind_is_supported(plan.kind)
            || !self.plan_is_fresh(&plan)
            || !self.character_plan_preserves_dependents(&plan)
        {
            return Err(ProjectEditError::StalePlan);
        }
        if plan.normalized_no_op {
            if plan.predicted_changed_entities != 0 {
                return Err(ProjectEditError::StalePlan);
            }
            self.status = ProjectEditStatus::ProjectNoOp;
            self.committed_plan = Some(plan);
            return Ok(self.status);
        }
        if plan.predicted_changed_entities == 0 {
            return Err(ProjectEditError::StalePlan);
        }
        self.advance_actual_edit(plan);
        Ok(self.status)
    }

    fn advance_actual_edit(&mut self, plan: ProjectEditPlan) {
        self.status = ProjectEditStatus::ProjectCommitted;
        self.head_revision += 1;
        match plan.kind {
            ProjectEditKind::ProjectSettingsEdit => self.settings_revision += 1,
            ProjectEditKind::SceneCreate
            | ProjectEditKind::SceneUpdate
            | ProjectEditKind::SceneDelete
            | ProjectEditKind::TimelineCreate
            | ProjectEditKind::TimelineUpdate
            | ProjectEditKind::TimelineDelete => self.scene_topology_revision += 1,
            ProjectEditKind::CharacterCreate
            | ProjectEditKind::CharacterUpdate
            | ProjectEditKind::CharacterDelete => self.character_catalog_revision += 1,
            _ => {}
        }
        if plan.kind == ProjectEditKind::CharacterUpdate {
            match plan.character_setting_class {
                CharacterSettingClass::CharacterName => self.character_name_digest += 1,
                CharacterSettingClass::CharacterGroup => self.character_group_digest += 1,
                CharacterSettingClass::CharacterVoiceType => self.character_voice_type_digest += 1,
                CharacterSettingClass::CharacterVoiceParameters => {
                    self.character_voice_parameters_digest += 1;
                }
                CharacterSettingClass::CharacterPortraitConfiguration => {
                    self.character_portrait_configuration_digest += 1;
                }
                CharacterSettingClass::NoCharacterSetting => {}
            }
        }
        match plan.kind {
            ProjectEditKind::SceneCreate => self.scene_count += 1,
            ProjectEditKind::SceneDelete => self.scene_count -= 1,
            ProjectEditKind::TimelineCreate => self.timeline_count += 1,
            ProjectEditKind::TimelineDelete => self.timeline_count -= 1,
            ProjectEditKind::CharacterCreate => self.character_count += 1,
            ProjectEditKind::CharacterDelete => {
                self.character_count -= 1;
                self.character_dependent_count = 0;
                self.managed_character_dependent_count = 0;
                self.dependent_set_digest += 1;
            }
            ProjectEditKind::NativeTemplateInstantiate => {
                self.instantiated_template_items += plan.predicted_changed_entities;
            }
            _ => {}
        }
        self.derived_epoch += 1;
        self.project_mutation_count += 1;
        self.canonical_commit_count += 1;
        self.committed_plan = Some(plan);
    }

    fn reject(
        &mut self,
        kind: ProjectEditKind,
        error: ProjectEditError,
    ) -> Result<ProjectEditPlan, ProjectEditError> {
        self.status = ProjectEditStatus::ProjectRejected;
        self.last_rejected_kind = kind;
        self.plan = None;
        Err(error)
    }

    fn require_idle(&self) -> Result<(), ProjectEditError> {
        if self.status == ProjectEditStatus::ProjectIdle {
            Ok(())
        } else {
            Err(ProjectEditError::UnexpectedStatus)
        }
    }

    fn kind_is_supported(&self, kind: ProjectEditKind) -> bool {
        match kind {
            ProjectEditKind::ProjectSettingsEdit => self.settings_mutation_supported,
            ProjectEditKind::SceneCreate
            | ProjectEditKind::SceneUpdate
            | ProjectEditKind::SceneDelete => self.scene_mutation_supported,
            ProjectEditKind::TimelineCreate
            | ProjectEditKind::TimelineUpdate
            | ProjectEditKind::TimelineDelete => self.timeline_mutation_supported,
            ProjectEditKind::CharacterCreate
            | ProjectEditKind::CharacterUpdate
            | ProjectEditKind::CharacterDelete => self.character_mutation_supported,
            ProjectEditKind::NativeTemplateInstantiate => self.template_mutation_supported,
            _ => false,
        }
    }

    fn target_revision_for(&self, kind: ProjectEditKind) -> i64 {
        match kind {
            ProjectEditKind::ProjectSettingsEdit => self.settings_revision,
            ProjectEditKind::SceneCreate
            | ProjectEditKind::SceneUpdate
            | ProjectEditKind::SceneDelete
            | ProjectEditKind::TimelineCreate
            | ProjectEditKind::TimelineUpdate
            | ProjectEditKind::TimelineDelete => self.scene_topology_revision,
            ProjectEditKind::CharacterCreate
            | ProjectEditKind::CharacterUpdate
            | ProjectEditKind::CharacterDelete => self.character_catalog_revision,
            ProjectEditKind::NativeTemplateInstantiate
            | ProjectEditKind::NativeTemplateDefinitionEdit => self.template_catalog_revision,
            ProjectEditKind::NoProjectEdit => -1,
        }
    }

    fn dependent_digest_for(&self, kind: ProjectEditKind) -> i64 {
        match kind {
            ProjectEditKind::SceneDelete => self.scene_reference_set_digest,
            ProjectEditKind::TimelineDelete => self.timeline_reference_set_digest,
            ProjectEditKind::CharacterUpdate | ProjectEditKind::CharacterDelete => {
                self.dependent_set_digest
            }
            _ => 0,
        }
    }

    fn dependent_count_for(&self, kind: ProjectEditKind) -> i64 {
        match kind {
            ProjectEditKind::SceneDelete => self.referenced_scene_count,
            ProjectEditKind::TimelineDelete => self.referenced_timeline_count,
            ProjectEditKind::CharacterUpdate | ProjectEditKind::CharacterDelete => {
                self.character_dependent_count
            }
            _ => 0,
        }
    }

    fn seal_plan(
        &self,
        kind: ProjectEditKind,
        setting_class: ProjectSettingClass,
        character_setting_class: CharacterSettingClass,
        policy: DependentPolicy,
        changed_entities: i64,
        normalized_no_op: bool,
    ) -> ProjectEditPlan {
        let character_update = kind == ProjectEditKind::CharacterUpdate;
        ProjectEditPlan {
            kind,
            setting_class,
            character_setting_class,
            character_name_preimage_digest: character_update.then_some(self.character_name_digest),
            character_group_preimage_digest: character_update
                .then_some(self.character_group_digest),
            character_voice_type_preimage_digest: character_update
                .then_some(self.character_voice_type_digest),
            character_voice_parameters_preimage_digest: character_update
                .then_some(self.character_voice_parameters_digest),
            character_portrait_configuration_preimage_digest: character_update
                .then_some(self.character_portrait_configuration_digest),
            unknown_character_configuration_preimage_digest: character_update
                .then_some(self.unknown_character_configuration_digest),
            digest: self.plan.as_ref().map_or(0, |plan| plan.digest) + 1,
            base_revision: self.head_revision,
            source_fingerprint: self.source_fingerprint,
            edit_surface_revision: self.edit_surface_revision,
            capability_revision: self.capability_revision,
            target_resource_revision: self.target_revision_for(kind),
            descriptor_catalog_digest: self.descriptor_catalog_revision,
            dependent_set_digest: self.dependent_digest_for(kind),
            dependent_count: self.dependent_count_for(kind),
            dependent_policy: policy,
            template_content_digest: if kind == ProjectEditKind::NativeTemplateInstantiate {
                self.template_content_digest
            } else {
                0
            },
            expansion_digest: if kind == ProjectEditKind::NativeTemplateInstantiate {
                self.template_expansion_digest
            } else {
                0
            },
            predicted_changed_entities: changed_entities,
            normalized_no_op,
        }
    }

    fn plan_is_fresh(&self, plan: &ProjectEditPlan) -> bool {
        plan.base_revision == self.head_revision
            && plan.source_fingerprint == self.source_fingerprint
            && plan.edit_surface_revision == self.edit_surface_revision
            && plan.capability_revision == self.capability_revision
            && plan.target_resource_revision == self.target_revision_for(plan.kind)
            && plan.descriptor_catalog_digest == self.descriptor_catalog_revision
            && plan.dependent_set_digest == self.dependent_digest_for(plan.kind)
            && plan.dependent_count == self.dependent_count_for(plan.kind)
            && (plan.kind != ProjectEditKind::CharacterUpdate
                || (plan.character_setting_class != CharacterSettingClass::NoCharacterSetting
                    && plan.character_name_preimage_digest == Some(self.character_name_digest)
                    && plan.character_group_preimage_digest == Some(self.character_group_digest)
                    && plan.character_voice_type_preimage_digest
                        == Some(self.character_voice_type_digest)
                    && plan.character_voice_parameters_preimage_digest
                        == Some(self.character_voice_parameters_digest)
                    && plan.character_portrait_configuration_preimage_digest
                        == Some(self.character_portrait_configuration_digest)
                    && plan.unknown_character_configuration_preimage_digest
                        == Some(self.unknown_character_configuration_digest)))
            && (plan.kind != ProjectEditKind::NativeTemplateInstantiate
                || (self.template_descriptor_available
                    && self.template_produced_kinds_supported
                    && plan.template_content_digest == self.template_content_digest
                    && plan.expansion_digest == self.template_expansion_digest))
    }

    fn character_plan_preserves_dependents(&self, plan: &ProjectEditPlan) -> bool {
        !matches!(
            plan.kind,
            ProjectEditKind::CharacterUpdate | ProjectEditKind::CharacterDelete
        ) || plan.dependent_count == 0
            || (plan.dependent_policy == DependentPolicy::RewriteManagedDependents
                && self.managed_character_dependent_count == plan.dependent_count
                && self.character_dependent_count == plan.dependent_count
                && plan.dependent_set_digest == self.dependent_set_digest)
    }
}

impl ProjectEditSession {
    /// Number of dependents preserved by the last committed character edit.
    #[must_use]
    pub fn preserved_dependent_items(&self) -> i64 {
        self.committed_plan.as_ref().map_or(0, |plan| {
            if matches!(
                plan.kind,
                ProjectEditKind::CharacterUpdate | ProjectEditKind::CharacterDelete
            ) {
                plan.dependent_count
            } else {
                0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_kind(
        session: &mut ProjectEditSession,
        kind: ProjectEditKind,
        setting: ProjectSettingClass,
        character: CharacterSettingClass,
        policy: DependentPolicy,
        changed: i64,
        no_op: bool,
    ) {
        session
            .stage(kind, setting, character, policy, changed, no_op)
            .expect("stage");
        session.approve().expect("approve");
        session.commit().expect("commit");
    }

    #[test]
    fn unavailable_families_fail_closed_without_a_plan() {
        let mut session = ProjectEditSession::initial();
        let err = session
            .stage(
                ProjectEditKind::ProjectSettingsEdit,
                ProjectSettingClass::CanvasSize,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::NoDependentPolicy,
                1,
                false,
            )
            .expect_err("settings");
        assert_eq!(
            err,
            ProjectEditError::UnsupportedFamily(ProjectEditKind::ProjectSettingsEdit)
        );
        assert_eq!(session.status, ProjectEditStatus::ProjectRejected);
        assert_eq!(
            session.last_rejected_kind,
            ProjectEditKind::ProjectSettingsEdit
        );
        assert!(session.plan.is_none());
        assert_eq!(session.project_mutation_count, 0);

        let mut timeline = ProjectEditSession::initial();
        let err = timeline
            .stage(
                ProjectEditKind::TimelineUpdate,
                ProjectSettingClass::NoProjectSetting,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::NoDependentPolicy,
                1,
                false,
            )
            .expect_err("timeline");
        assert_eq!(
            err,
            ProjectEditError::UnsupportedFamily(ProjectEditKind::TimelineUpdate)
        );
        assert_eq!(timeline.last_rejected_kind, ProjectEditKind::TimelineUpdate);
    }

    #[test]
    fn actual_settings_edit_invalidates_derived_epochs_and_noop_does_not() {
        let mut session = ProjectEditSession::initial();
        session
            .enable(ProjectEditCapability::SettingsMutation)
            .expect("enable");
        commit_kind(
            &mut session,
            ProjectEditKind::ProjectSettingsEdit,
            ProjectSettingClass::CanvasSize,
            CharacterSettingClass::NoCharacterSetting,
            DependentPolicy::NoDependentPolicy,
            1,
            false,
        );
        assert_eq!(session.status, ProjectEditStatus::ProjectCommitted);
        assert_eq!(session.settings_revision, 6);
        assert_eq!(session.derived_epoch, 1);
        assert!(session.derived_epoch > session.checkpoint_epoch);
        assert!(session.derived_epoch > session.inspection_epoch);
        assert!(session.derived_epoch > session.render_epoch);
        assert!(session.derived_epoch > session.composition_epoch);
        assert!(session.derived_epoch > session.timeline_plan_epoch);
        assert_eq!(session.head_revision, INITIAL_HEAD_REVISION + 1);

        let mut no_op = ProjectEditSession::initial();
        no_op
            .enable(ProjectEditCapability::SettingsMutation)
            .expect("enable");
        commit_kind(
            &mut no_op,
            ProjectEditKind::ProjectSettingsEdit,
            ProjectSettingClass::CanvasSize,
            CharacterSettingClass::NoCharacterSetting,
            DependentPolicy::NoDependentPolicy,
            0,
            true,
        );
        assert_eq!(no_op.status, ProjectEditStatus::ProjectNoOp);
        assert_eq!(no_op.project_mutation_count, 0);
        assert_eq!(no_op.head_revision, INITIAL_HEAD_REVISION);
        assert_eq!(no_op.derived_epoch, 0);
    }

    #[test]
    fn character_update_is_one_setting_class_and_preserves_the_rest() {
        let classes = [
            (
                CharacterSettingClass::CharacterName,
                102,
                102,
                103,
                104,
                105,
            ),
            (
                CharacterSettingClass::CharacterGroup,
                101,
                103,
                103,
                104,
                105,
            ),
            (
                CharacterSettingClass::CharacterVoiceType,
                101,
                102,
                104,
                104,
                105,
            ),
            (
                CharacterSettingClass::CharacterVoiceParameters,
                101,
                102,
                103,
                105,
                105,
            ),
            (
                CharacterSettingClass::CharacterPortraitConfiguration,
                101,
                102,
                103,
                104,
                106,
            ),
        ];
        for (class, name, group, voice, params, portrait) in classes {
            let mut session = ProjectEditSession::initial();
            session
                .enable(ProjectEditCapability::CharacterMutation)
                .expect("enable");
            commit_kind(
                &mut session,
                ProjectEditKind::CharacterUpdate,
                ProjectSettingClass::NoProjectSetting,
                class,
                DependentPolicy::RewriteManagedDependents,
                3,
                false,
            );
            let plan = session.committed_plan.as_ref().expect("plan");
            assert_eq!(plan.character_setting_class, class);
            assert_eq!(session.character_name_digest, name);
            assert_eq!(session.character_group_digest, group);
            assert_eq!(session.character_voice_type_digest, voice);
            assert_eq!(session.character_voice_parameters_digest, params);
            assert_eq!(session.character_portrait_configuration_digest, portrait);
            assert_eq!(session.unknown_character_configuration_digest, 199);
            assert_eq!(session.preserved_dependent_items(), 2);
        }
    }

    #[test]
    fn template_instantiate_is_digest_bound_and_definition_stays_closed() {
        let mut session = ProjectEditSession::initial();
        session
            .enable(ProjectEditCapability::TemplateMutation)
            .expect("enable");
        commit_kind(
            &mut session,
            ProjectEditKind::NativeTemplateInstantiate,
            ProjectSettingClass::NoProjectSetting,
            CharacterSettingClass::NoCharacterSetting,
            DependentPolicy::NoDependentPolicy,
            2,
            false,
        );
        assert_eq!(session.instantiated_template_items, 2);
        assert_eq!(
            session
                .committed_plan
                .as_ref()
                .map(|plan| plan.expansion_digest),
            Some(82)
        );

        let mut definition = ProjectEditSession::initial();
        definition
            .enable(ProjectEditCapability::TemplateMutation)
            .expect("enable");
        let err = definition
            .stage(
                ProjectEditKind::NativeTemplateDefinitionEdit,
                ProjectSettingClass::NoProjectSetting,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::NoDependentPolicy,
                1,
                false,
            )
            .expect_err("definition");
        assert_eq!(err, ProjectEditError::TemplateDefinitionUnavailable);
        assert_eq!(definition.instantiated_template_items, 0);

        let mut drift = ProjectEditSession::initial();
        drift
            .enable(ProjectEditCapability::TemplateMutation)
            .expect("enable");
        drift
            .stage(
                ProjectEditKind::NativeTemplateInstantiate,
                ProjectSettingClass::NoProjectSetting,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::NoDependentPolicy,
                2,
                false,
            )
            .expect("stage");
        drift.approve().expect("approve");
        drift.drift_template();
        drift.mark_conflict().expect("conflict");
        assert_eq!(drift.status, ProjectEditStatus::ProjectConflicted);
        assert_eq!(drift.canonical_commit_count, 0);
    }

    #[test]
    fn referenced_deletes_and_unknown_character_dependents_reject() {
        let mut scene = ProjectEditSession::initial();
        scene
            .enable(ProjectEditCapability::SceneMutation)
            .expect("enable");
        let err = scene
            .stage(
                ProjectEditKind::SceneDelete,
                ProjectSettingClass::NoProjectSetting,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::RejectWhenReferenced,
                1,
                false,
            )
            .expect_err("scene");
        assert_eq!(err, ProjectEditError::ReferencedDelete);
        assert_eq!(scene.scene_count, 2);

        let mut unknown = ProjectEditSession::initial();
        unknown
            .enable(ProjectEditCapability::CharacterMutation)
            .expect("enable");
        unknown.select_character_with_unknown_dependent();
        let err = unknown
            .stage(
                ProjectEditKind::CharacterUpdate,
                ProjectSettingClass::NoProjectSetting,
                CharacterSettingClass::CharacterName,
                DependentPolicy::RewriteManagedDependents,
                1,
                false,
            )
            .expect_err("unknown");
        assert_eq!(err, ProjectEditError::UnknownCharacterDependents);
    }

    #[test]
    fn observation_is_read_only_and_surface_drift_invalidates_approval() {
        let mut session = ProjectEditSession::initial();
        session.observe();
        session.observe();
        assert_eq!(session.observation_count, 2);
        assert_eq!(session.project_mutation_count, 0);
        assert_eq!(session.head_revision, INITIAL_HEAD_REVISION);

        session
            .enable(ProjectEditCapability::SettingsMutation)
            .expect("enable");
        session
            .stage(
                ProjectEditKind::ProjectSettingsEdit,
                ProjectSettingClass::FrameRate,
                CharacterSettingClass::NoCharacterSetting,
                DependentPolicy::NoDependentPolicy,
                1,
                false,
            )
            .expect("stage");
        session.approve().expect("approve");
        session.drift_surface();
        session.mark_conflict().expect("conflict");
        assert_eq!(session.status, ProjectEditStatus::ProjectConflicted);
        assert_eq!(session.canonical_commit_count, 0);
    }
}
