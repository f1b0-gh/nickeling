use crate::{
    backend::*,
    composition::{Compiled, CompiledUnit, Validator},
    model::{Gate, Management, Unit},
    require,
    store::Store,
    Error, Result,
};
use nickel_util::files;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
pub type Registry = BTreeMap<String, Box<dyn Backend>>;
pub struct Options {
    pub now: u64,
    pub execute: bool,
    pub prepare: bool,
    pub retry_failed: bool,
    pub approved_operation: Option<String>,
    pub cancelled: Arc<AtomicBool>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            now: 1,
            execute: true,
            prepare: false,
            retry_failed: false,
            approved_operation: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitState {
    pub revision: String,
    pub artifact_sha256: String,
    #[serde(default)]
    pub applied_sha256: String,
    pub target_sha256: String,
    pub rendered: bool,
    pub applied: bool,
    pub readiness: Readiness,
    pub drifted: bool,
    pub retained: bool,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub removing: bool,
    pub failure: Option<String>,
    pub waiting: Option<String>,
    pub attempts: u32,
    pub next_retry: u64,
    pub outputs: BTreeMap<String, OutputValue>,
    pub inventory: Value,
    pub prepared: Option<PreparedChange>,
    pub definition: Option<Unit>,
    pub effective: Option<CompiledUnit>,
}
impl UnitState {
    pub fn public(&self) -> Value {
        json!({"revision":self.revision,"artifactSha256":self.artifact_sha256,
 "conditions":{"Rendered":self.rendered,"Applied":self.applied,"Ready":self.readiness==Readiness::Ready&&self.applied,"Drifted":self.drifted,"Failed":self.failure.is_some()},
 "readiness":self.readiness,"retained":self.retained,"deleted":self.deleted,"failure":self.failure,"waiting":self.waiting,"attempts":self.attempts,"nextRetry":self.next_retry,
 "outputs":self.outputs.iter().map(|(name,o)|(name.clone(),if o.sensitive{json!({"sensitive":true,"reference":o.reference})}else{json!({"sensitive":false,"value":o.value})})).collect::<BTreeMap<_,_>>(),
 "change":self.prepared.as_ref().map(|p|json!({"operation":p.operation,"revision":p.revision,"summary":p.summary}))})
    }
    fn failed(&mut self, error: &Error, now: u64) {
        self.failure = Some(error.to_string());
        self.applied = false;
        self.outputs.clear();
        self.attempts = self.attempts.saturating_add(1);
        self.next_retry = now.saturating_add(2u64.pow(self.attempts.min(8)));
    }
}
fn satisfies(state: &UnitState, revision: &str, gate: Gate) -> bool {
    state.revision == revision
        && state.applied
        && !state.drifted
        && state.failure.is_none()
        && !state.retained
        && (gate == Gate::Applied || state.readiness == Readiness::Ready)
}
fn resolve(
    store: &Store,
    compiled: &Compiled,
    unit: &Unit,
) -> Result<std::result::Result<Value, String>> {
    for dependency in &unit.depends_on {
        if !store
            .unit(&compiled.instance.id, &dependency.unit)?
            .is_some_and(|state| satisfies(&state, &compiled.revision, dependency.condition))
        {
            return Ok(Err(format!(
                "Waiting for {} at current revision",
                dependency.unit
            )));
        }
    }
    let mut configuration = unit.configuration.clone();
    for binding in &unit.bindings {
        let Some(state) = store.unit(&compiled.instance.id, &binding.from_unit)? else {
            return Ok(Err(format!("Waiting for {}", binding.from_unit)));
        };
        if !satisfies(&state, &compiled.revision, binding.condition) {
            return Ok(Err(format!(
                "Waiting for {} at current revision",
                binding.from_unit
            )));
        }
        let Some(output) = state.outputs.get(&binding.output) else {
            return Ok(Err(format!(
                "Waiting for output {}.{}",
                binding.from_unit, binding.output
            )));
        };
        require(!output.sensitive,"Sensitive output bindings require an explicit protected-reference integration; plaintext transfer is disabled")?;
        let Some(value) = &output.value else {
            return Ok(Err("Output value unavailable".into()));
        };
        let field = configuration
            .pointer_mut(&binding.to)
            .ok_or_else(|| Error::Invalid("Binding target disappeared".into()))?;
        *field = value.clone();
    }
    Ok(Ok(configuration))
}
fn accept_observation(state: &mut UnitState, mut observation: Observation) {
    for output in observation.outputs.values_mut() {
        if output.sensitive {
            output.value = None;
        }
    }
    state.outputs = observation.outputs;
    state.inventory = observation.inventory;
    state.readiness = observation.readiness;
    state.drifted = observation.drifted;
    state.applied = observation.exists && !observation.drifted;
    state.failure = (observation.readiness == Readiness::Failed)
        .then(|| "Workload reports a failed rollout".into());
    state.waiting = None;
}
pub fn reconcile(
    store: &mut Store,
    compiled: &Compiled,
    validator: &dyn Validator,
    backends: &mut Registry,
    options: &Options,
) -> Result<()> {
    compiled.verify()?;
    store.ensure_revision(&compiled.instance.id, &compiled.revision)?;
    let order = compiled
        .instance
        .validate(&backends.keys().cloned().collect())?;
    let removal = store.removal(&compiled.instance.id)?;
    if removal > 0 {
        return remove_units(store, compiled, backends, options, removal == 2);
    }
    for id in order {
        if options.cancelled.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        let unit = &compiled.instance.units[&id];
        let mut state = store.unit(&compiled.instance.id, &id)?.unwrap_or_default();
        if state.revision != compiled.revision {
            state.revision = compiled.revision.clone();
            state.applied = false;
            state.prepared = None;
            state.attempts = 0;
            state.next_retry = 0;
            state.outputs.clear();
            state.failure = None;
        }
        if options.retry_failed {
            state.attempts = 0;
            state.next_retry = 0;
        }
        if state.attempts >= 5 || state.next_retry > options.now {
            continue;
        }
        let result = (|| -> Result<()> {
            let configuration = match resolve(store, compiled, unit)? {
                Ok(value) => value,
                Err(waiting) => {
                    state.waiting = Some(waiting);
                    state.applied = false;
                    state.rendered = false;
                    state.outputs.clear();
                    return Ok(());
                }
            };
            let effective = validator.validate(unit, &configuration, &compiled.units[&id])?;
            let artifact_sha256 = files::hash(&serde_json::to_vec(&effective.artifact)?);
            let backend = backends
                .get_mut(&unit.target)
                .ok_or_else(|| Error::Invalid("Target backend unavailable".into()))?;
            require(
                backend.kind() == unit.backend,
                "Target backend kind does not match unit",
            )?;
            let target_sha256 = backend.fingerprint()?;
            if !state.target_sha256.is_empty() && state.target_sha256 != target_sha256 {
                return Err(Error::Conflict(
                    "Target changed for an owned unit; explicit migration required".into(),
                ));
            }
            if let Some(binding) = backend.state_binding()? {
                store.claim(&binding, &compiled.instance.id, &id)?;
            }
            let running = store.running(&compiled.instance.id, &id)?;
            let context = Context {
                owner: store.owner.clone(),
                instance: compiled.instance.id.clone(),
                unit_id: id.clone(),
                revision: compiled.revision.clone(),
                unit: unit.clone(),
                artifact: effective.artifact.clone(),
                artifact_sha256: artifact_sha256.clone(),
                inventory: state.inventory.clone(),
                interrupted: running.clone(),
                work: store.work(&compiled.instance.id, &id)?,
                engine_version: effective.engine_version.clone(),
                provider_lock: effective.provider_lock.clone().map(String::into_bytes),
            };
            let observed = backend.observe(&context)?;
            if let Some(interrupted) = running {
                state.inventory = observed.inventory.clone();
                // Persist recovered ownership before closing the recovery journal.
                store.save_unit(&compiled.instance.id, &id, &state)?;
                store.finish(&interrupted.operation, "recovered")?;
                state.prepared = None;
            }
            let artifact_changed = state.applied_sha256 != artifact_sha256;
            state.target_sha256 = target_sha256.clone();
            state.artifact_sha256 = artifact_sha256;
            state.rendered = true;
            state.retained = false;
            state.deleted = false;
            state.removing = false;
            state.definition = Some(unit.clone());
            state.effective = Some(effective);
            let changed = !observed.exists
                || observed.drifted
                || (artifact_changed
                    && !context
                        .interrupted
                        .as_ref()
                        .is_some_and(|p| p.artifact_sha256 == context.artifact_sha256));
            if let Some(approved) = &options.approved_operation {
                if let Some(plan) = state.prepared.as_ref().filter(|p| &p.operation == approved) {
                    require(
                        changed && plan.preconditions == observed.preconditions,
                        "Approved plan is stale; prepare and review a fresh plan",
                    )?;
                }
            }
            if !changed {
                accept_observation(&mut state, observed);
                state.applied_sha256 = context.artifact_sha256.clone();
                state.attempts = 0;
                state.next_retry = 0;
                state.prepared = None;
                return Ok(());
            }
            state.applied = false;
            state.drifted = observed.drifted;
            state.inventory = observed.inventory.clone();
            state.readiness = observed.readiness;
            state.outputs.clear();
            let approved = options.approved_operation.as_ref();
            let selected = approved
                .is_none_or(|id| state.prepared.as_ref().is_some_and(|p| p.operation == *id));
            if !selected
                || !(options.prepare
                    || (options.execute && compiled.instance.policy.mode == Management::Manage))
            {
                return Ok(());
            }
            let change = if let Some(previous) = state.prepared.as_ref().filter(|p| {
                p.revision == compiled.revision
                    && p.artifact_sha256 == context.artifact_sha256
                    && p.target_sha256 == target_sha256
                    && p.preconditions == observed.preconditions
            }) {
                previous.clone()
            } else {
                require(
                    approved.is_none(),
                    "Approved plan is stale; prepare and review a fresh plan",
                )?;
                let token = store.token()?;
                let prepared = backend.prepare_change(&context, &observed, &token)?;
                require(
                    prepared.operation == token
                        && prepared.revision == context.revision
                        && prepared.artifact_sha256 == context.artifact_sha256
                        && prepared.target_sha256 == target_sha256,
                    "Backend returned an incorrectly bound plan",
                )?;
                store.record(&compiled.instance.id, &id, &prepared, "prepared")?;
                prepared
            };
            state.prepared = Some(change.clone());
            store.save_unit(&compiled.instance.id, &id, &state)?;
            if !options.execute {
                return Ok(());
            }
            require(
                compiled.instance.policy.mode == Management::Manage,
                "Instance is observe-only",
            )?;
            require(
                change.summary.deletes == 0 || compiled.instance.policy.allow_delete,
                "Policy denies deletion or pruning",
            )?;
            require(
                change.summary.replacements == 0 || compiled.instance.policy.allow_replace,
                "Policy denies replacement",
            )?;
            if options.cancelled.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            store.ensure_revision(&compiled.instance.id, &compiled.revision)?;
            store.record(&compiled.instance.id, &id, &change, "running")?;
            let after = backend.execute_change(&context, &change)?;
            accept_observation(&mut state, after);
            state.applied_sha256 = context.artifact_sha256.clone();
            state.prepared = None;
            state.attempts = 0;
            state.next_retry = 0;
            store.save_unit(&compiled.instance.id, &id, &state)?;
            store.finish(&change.operation, "succeeded")?;
            Ok(())
        })();
        if let Err(error) = result {
            if matches!(error, Error::Cancelled) {
                return Err(error);
            }
            state.failed(&error, options.now);
        }
        store.save_unit(&compiled.instance.id, &id, &state)?;
    }
    let removed = store
        .units(&compiled.instance.id)?
        .into_iter()
        .filter(|(id, _)| !compiled.instance.units.contains_key(id))
        .collect();
    remove_selected(
        store,
        compiled,
        backends,
        options,
        compiled.instance.policy.deletion == crate::model::Deletion::Delete,
        removed,
    )
}
fn remove_units(
    store: &mut Store,
    compiled: &Compiled,
    backends: &mut Registry,
    options: &Options,
    delete: bool,
) -> Result<()> {
    let states = store.units(&compiled.instance.id)?;
    remove_selected(store, compiled, backends, options, delete, states)
}
fn remove_selected(
    store: &mut Store,
    compiled: &Compiled,
    backends: &mut Registry,
    options: &Options,
    delete: bool,
    mut states: BTreeMap<String, UnitState>,
) -> Result<()> {
    // Remove dependents before their providers. Retained historical definitions
    // participate even when they disappeared from the current composition.
    let mut order = Vec::new();
    let mut remaining: std::collections::BTreeSet<_> = states.keys().cloned().collect();
    while !remaining.is_empty() {
        let leaf = remaining
            .iter()
            .find(|id| {
                !remaining.iter().any(|other| {
                    states[other].definition.as_ref().is_some_and(|u| {
                        u.depends_on.iter().any(|d| &d.unit == *id)
                            || u.bindings.iter().any(|b| &b.from_unit == *id)
                    })
                })
            })
            .cloned()
            .ok_or_else(|| {
                Error::Conflict("Retained dependency cycle requires explicit migration".into())
            })?;
        remaining.remove(&leaf);
        order.push(leaf);
    }
    for id in order {
        let mut state = states
            .remove(&id)
            .ok_or_else(|| Error::Invalid("Missing removal state".into()))?;
        if state.deleted {
            continue;
        }
        let selected = options.approved_operation.as_ref().is_none_or(|operation| {
            state.removing
                && state
                    .prepared
                    .as_ref()
                    .is_some_and(|p| &p.operation == operation)
        });
        // A selected upstream deletion must wait for all removed dependents.
        // Applying a live-unit plan never authorizes this removal pass.
        if options.approved_operation.is_some() && !selected {
            break;
        }
        if state.revision != compiled.revision || !state.removing {
            state.prepared = None;
            state.attempts = 0;
            state.next_retry = 0;
            state.failure = None;
        }
        state.revision = compiled.revision.clone();
        state.removing = true;
        state.applied = false;
        state.outputs.clear();
        if !delete {
            state.retained = !state.deleted;
            store.save_unit(&compiled.instance.id, &id, &state)?;
            continue;
        }
        if options.cancelled.load(Ordering::SeqCst) {
            return Err(Error::Cancelled);
        }
        if options.retry_failed {
            state.attempts = 0;
            state.next_retry = 0;
        }
        if state.attempts >= 5 || state.next_retry > options.now {
            break;
        }
        let result = (|| -> Result<()> {
            // A unit that never reached domain validation has no external inventory.
            if state.definition.is_none() && state.inventory.is_null() {
                state.deleted = true;
                return Ok(());
            }
            let unit = state
                .definition
                .as_ref()
                .ok_or_else(|| Error::Invalid("Missing retained unit definition".into()))?;
            let artifact = state
                .effective
                .as_ref()
                .ok_or_else(|| Error::Invalid("Missing retained artifact".into()))?;
            let backend = backends
                .get_mut(&unit.target)
                .ok_or_else(|| Error::Invalid("Missing retained target".into()))?;
            require(
                backend.fingerprint()? == state.target_sha256,
                "Target changed before removal",
            )?;
            let context = Context {
                owner: store.owner.clone(),
                instance: compiled.instance.id.clone(),
                unit_id: id.clone(),
                revision: compiled.revision.clone(),
                unit: unit.clone(),
                artifact: artifact.artifact.clone(),
                artifact_sha256: state.artifact_sha256.clone(),
                inventory: state.inventory.clone(),
                interrupted: store.running(&compiled.instance.id, &id)?,
                work: store.work(&compiled.instance.id, &id)?,
                engine_version: artifact.engine_version.clone(),
                provider_lock: artifact.provider_lock.clone().map(String::into_bytes),
            };
            let observed = backend.observe_removal(&context)?;
            state.inventory = observed.inventory.clone();
            store.save_unit(&compiled.instance.id, &id, &state)?;
            if let Some(previous) = &context.interrupted {
                store.finish(&previous.operation, "recovered")?;
                state.prepared = None;
            }
            if !observed.exists
                && (options.approved_operation.is_none() || context.interrupted.is_some())
            {
                state.deleted = true;
                state.retained = false;
                state.prepared = None;
                state.failure = None;
                state.attempts = 0;
                state.next_retry = 0;
                return Ok(());
            }
            if !(options.prepare || options.execute) {
                return Ok(());
            }
            let change = if let Some(previous) = state.prepared.as_ref().filter(|p| {
                p.revision == compiled.revision
                    && p.artifact_sha256 == context.artifact_sha256
                    && p.target_sha256 == state.target_sha256
                    && p.preconditions == observed.preconditions
            }) {
                previous.clone()
            } else {
                require(
                    options.approved_operation.is_none(),
                    "Approved removal plan is stale; prepare and review a fresh plan",
                )?;
                let token = store.token()?;
                let prepared = backend.prepare_removal(&context, &observed, &token)?;
                require(
                    prepared.operation == token
                        && prepared.revision == context.revision
                        && prepared.artifact_sha256 == context.artifact_sha256
                        && prepared.target_sha256 == state.target_sha256,
                    "Backend returned an incorrectly bound removal plan",
                )?;
                store.record(&compiled.instance.id, &id, &prepared, "prepared")?;
                prepared
            };
            state.prepared = Some(change.clone());
            state.failure = None;
            store.save_unit(&compiled.instance.id, &id, &state)?;
            if !options.execute {
                return Ok(());
            }
            require(
                compiled.instance.policy.allow_delete
                    && compiled.instance.policy.mode == Management::Manage,
                "Deletion requires explicit execution policy",
            )?;
            if options.cancelled.load(Ordering::SeqCst) {
                return Err(Error::Cancelled);
            }
            store.ensure_revision(&compiled.instance.id, &compiled.revision)?;
            store.record(&compiled.instance.id, &id, &change, "running")?;
            let after = backend.execute_removal(&context, &change)?;
            state.inventory = after.inventory;
            state.retained = after.exists;
            state.deleted = !after.exists;
            state.failure = None;
            state.attempts = 0;
            state.next_retry = 0;
            store.save_unit(&compiled.instance.id, &id, &state)?;
            if state.deleted {
                state.prepared = None;
                store.save_unit(&compiled.instance.id, &id, &state)?;
                store.finish(&change.operation, "succeeded")?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if matches!(error, Error::Cancelled) {
                return Err(error);
            }
            state.failed(&error, options.now);
        }
        store.save_unit(&compiled.instance.id, &id, &state)?;
        // A pending or failed dependent protects its upstream resources.
        if !state.deleted {
            break;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollEvents {
    Changed,
    Quiet,
    Unavailable,
}

pub fn poll_events(
    store: &Store,
    compiled: &Compiled,
    backends: &mut Registry,
) -> Result<PollEvents> {
    let started = std::time::Instant::now();
    for state in store.units(&compiled.instance.id)?.values() {
        if started.elapsed() >= std::time::Duration::from_secs(3) {
            break;
        }
        if let Some(unit) = &state.definition {
            if let Some(backend) = backends.get_mut(&unit.target) {
                match backend.poll_events(&state.inventory) {
                    Ok(true) => return Ok(PollEvents::Changed),
                    Err(_) => return Ok(PollEvents::Unavailable),
                    Ok(false) => {}
                }
            }
        }
    }
    Ok(PollEvents::Quiet)
}
