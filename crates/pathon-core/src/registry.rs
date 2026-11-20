use pathon_api::{Negotiation, RenderFeature, Requirements, Role, SetupCtx, Shared, TaskPool};
use pathon_diag::Diagnostic;
use pathon_graph::{OrderNode, Stage, sort_nodes};
use pathon_rhi::{Backend, Caps, DeviceApi, DeviceRequest};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FeatureStatus {
    Active,
    Fallback { reason: String },
    Disabled { code: &'static str, reason: String },
}

impl FeatureStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active | Self::Fallback { .. })
    }
}

// What tools show about a registered feature.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FeatureInfo {
    pub name: &'static str,
    pub status: FeatureStatus,
    pub stage: Stage,
    pub priority: i32,
    pub role: Option<Role>,
    // Position in frame order; `None` when not active.
    pub order: Option<usize>,
}

pub(crate) struct FeatureEntry {
    pub(crate) feature: Box<dyn RenderFeature>,
    pub(crate) status: FeatureStatus,
}

// Registered render features in frame order, with the outcome of requirements, negotiation, roles and setup.
pub(crate) struct Registry {
    pub(crate) entries: Vec<FeatureEntry>,
    pub(crate) order: Vec<usize>,
    pub(crate) temporal_owner: Option<usize>,
}

impl Registry {
    pub(crate) fn new(features: Vec<Box<dyn RenderFeature>>) -> Self {
        let entries = features
            .into_iter()
            .map(|feature| FeatureEntry {
                feature,
                status: FeatureStatus::Active,
            })
            .collect();
        Self {
            entries,
            order: Vec::new(),
            temporal_owner: None,
        }
    }

    pub(crate) fn disable(&mut self, index: usize, diagnostic: Diagnostic) {
        pathon_diag::emit(&diagnostic);
        self.entries[index].status = FeatureStatus::Disabled {
            code: diagnostic.code,
            reason: diagnostic.message,
        };
    }

    // Phase 1, before the device exists. The first feature to name a backend wins; later conflicting ones are off.
    pub(crate) fn merge_requirements(&mut self, request: &mut DeviceRequest) {
        let mut backend_owner: Option<(Backend, &'static str)> = request.backend.map(|backend| (backend, "the host"));
        for index in 0..self.entries.len() {
            let requirements: Requirements = self.entries[index].feature.requirements();
            let name = self.entries[index].feature.name();
            if let Some(wanted) = requirements.backend {
                match backend_owner {
                    Some((owned, owner)) if owned != wanted => {
                        self.disable(
                            index,
                            Diagnostic::warning(
                                "F0201",
                                format!("'{name}' requires {wanted:?}, but {owner} already chose {owned:?}"),
                            )
                            .with_hint("pick one backend or unregister one of the features"),
                        );
                        continue;
                    }
                    Some(_) => {}
                    None => backend_owner = Some((wanted, name)),
                }
            }
            request.required_features |= requirements.required_features;
            request.optional_features |= requirements.optional_features;
            request.extensions.extend(requirements.extensions);
        }
        request.backend = backend_owner.map(|(backend, _)| backend);
    }

    // Phase 2, on the real capabilities, then order, roles and setup.
    pub(crate) fn activate(&mut self, device: &dyn DeviceApi, task_pool: &dyn TaskPool, shared: &mut Shared) {
        self.negotiate(device.caps());
        self.sort();
        self.assign_roles();
        let mut ctx = SetupCtx::new(device, task_pool, shared);
        for index in self.order.clone() {
            if let Err(error) = self.entries[index].feature.setup(&mut ctx) {
                let name = self.entries[index].feature.name();
                self.disable(
                    index,
                    Diagnostic::error("F0104", format!("'{name}' failed during setup: {error}")),
                );
            }
        }
        self.order.retain(|index| self.entries[*index].status.is_active());
    }

    pub(crate) fn reset(&mut self, device: &dyn DeviceApi, task_pool: &dyn TaskPool, shared: &mut Shared) {
        let mut ctx = SetupCtx::new(device, task_pool, shared);
        for index in self.order.clone() {
            if let Err(error) = self.entries[index].feature.on_device_reset(&mut ctx) {
                let name = self.entries[index].feature.name();
                self.disable(
                    index,
                    Diagnostic::error("F0104", format!("'{name}' failed after device reset: {error}")),
                );
            }
        }
        self.order.retain(|index| self.entries[*index].status.is_active());
    }

    fn negotiate(&mut self, caps: &Caps) {
        for index in 0..self.entries.len() {
            if !self.entries[index].status.is_active() {
                continue;
            }
            let name = self.entries[index].feature.name();
            match self.entries[index].feature.negotiate(caps) {
                Negotiation::Full => {}
                Negotiation::Fallback { reason } => {
                    pathon_diag::emit(&Diagnostic::note(
                        "F0103",
                        format!("'{name}' runs its fallback: {reason}"),
                    ));
                    self.entries[index].status = FeatureStatus::Fallback { reason };
                }
                Negotiation::Disabled { reason } => {
                    self.disable(
                        index,
                        Diagnostic::warning("F0101", format!("'{name}' disabled: {reason}")),
                    );
                }
            }
        }
    }

    fn sort(&mut self) {
        let constraints: Vec<(Vec<&'static str>, Vec<&'static str>)> = self
            .entries
            .iter()
            .map(|entry| (entry.feature.before().to_vec(), entry.feature.after().to_vec()))
            .collect();
        let nodes: Vec<OrderNode<'_>> = self
            .entries
            .iter()
            .zip(&constraints)
            .map(|(entry, (before, after))| OrderNode {
                name: entry.feature.name(),
                stage: entry.feature.stage(),
                priority: entry.feature.priority(),
                before,
                after,
                included: entry.status.is_active(),
            })
            .collect();
        match sort_nodes(&nodes, &[]) {
            Ok((order, warnings)) => {
                for warning in &warnings {
                    pathon_diag::emit(warning);
                }
                self.order = order;
            }
            Err(errors) => {
                for error in &errors {
                    pathon_diag::emit(error);
                }
                self.order = (0..self.entries.len())
                    .filter(|index| self.entries[*index].status.is_active())
                    .collect();
            }
        }
    }

    // The first feature in frame order keeps a role; later claimants are disabled.
    fn assign_roles(&mut self) {
        let mut holders: Vec<(Role, usize)> = Vec::new();
        for index in self.order.clone() {
            let Some(role) = self.entries[index].feature.role() else {
                continue;
            };
            if let Some((_, holder)) = holders.iter().find(|(held, _)| *held == role) {
                let name = self.entries[index].feature.name();
                let holder = self.entries[*holder].feature.name();
                self.disable(
                    index,
                    Diagnostic::warning(
                        "F0102",
                        format!("'{name}' claims role '{}', already held by '{holder}'", role.name()),
                    )
                    .with_hint("unregister one of the features"),
                );
                continue;
            }
            holders.push((role, index));
        }
        self.order.retain(|index| self.entries[*index].status.is_active());
        self.temporal_owner = holders
            .iter()
            .find(|(role, _)| *role == Role::TEMPORAL_RESOLVE)
            .map(|(_, index)| *index);
    }

    pub(crate) fn details(&self) -> Vec<FeatureInfo> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| FeatureInfo {
                name: entry.feature.name(),
                status: entry.status.clone(),
                stage: entry.feature.stage(),
                priority: entry.feature.priority(),
                role: entry.feature.role(),
                order: self.order.iter().position(|active| *active == index),
            })
            .collect()
    }

    pub(crate) fn statuses(&self) -> Vec<(&'static str, FeatureStatus)> {
        self.entries
            .iter()
            .map(|entry| (entry.feature.name(), entry.status.clone()))
            .collect()
    }
}
