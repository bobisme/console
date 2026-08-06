//! Bounded, named ECS queries and deterministic frame-to-frame deltas.
//!
//! Watches are host-side diagnostics. They deliberately retain only the last
//! bounded sample for each definition: enough to calculate a delta without
//! turning a long agent session into an unbounded entity log.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value, json};

pub const MAX_WATCHES: usize = 32;
pub const DEFAULT_ENTITY_DELTA_LIMIT: usize = 64;
pub const MAX_ENTITY_DELTA_LIMIT: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryDefinition {
    pub world: String,
    #[serde(rename = "with")]
    pub required: Vec<String>,
    pub select: BTreeMap<String, Vec<String>>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchDefinition {
    pub name: String,
    #[serde(flatten)]
    pub query: QueryDefinition,
    pub entity_delta_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchMetadata {
    #[serde(flatten)]
    pub definition: WatchDefinition,
    pub samples: u64,
    pub last_frame_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchBudgets {
    pub definitions: usize,
    pub entities: usize,
    pub entity_delta_ids: usize,
    pub projection_cells: usize,
    pub projected_string_bytes: usize,
    pub projected_value_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchDelta {
    pub comparable: bool,
    pub previous_frame_count: Option<u64>,
    pub frames: Option<u64>,
    pub alive: Option<i64>,
    pub matched: Option<i64>,
    pub returned: Option<i64>,
    pub component_counts: BTreeMap<String, i64>,
    pub spawned: Vec<u64>,
    pub despawned: Vec<u64>,
    pub spawned_truncated: bool,
    pub despawned_truncated: bool,
    /// False when either inspector page was truncated, so the ID lists only
    /// describe the returned prefix rather than the full matching set.
    pub entity_membership_complete: bool,
    /// True when membership is incomplete or either changed-ID list hit its
    /// explicit cap. Numeric count deltas remain exact in this case.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WatchSample {
    pub watch: String,
    pub definition: WatchDefinition,
    pub budgets: WatchBudgets,
    pub sample_index: u64,
    pub frame_count: u64,
    pub alive: u64,
    pub capacity: u64,
    pub component_type_count: u64,
    pub matched: u64,
    pub returned: u64,
    pub truncated: bool,
    pub budget_exhausted: bool,
    pub next_after: u64,
    pub component_counts: BTreeMap<String, u64>,
    pub entities: Vec<Value>,
    pub delta: WatchDelta,
}

#[derive(Debug, Clone)]
struct Baseline {
    frame_count: u64,
    alive: u64,
    matched: u64,
    returned: u64,
    component_counts: BTreeMap<String, u64>,
    entity_ids: BTreeSet<u64>,
    snapshot_truncated: bool,
}

#[derive(Debug, Clone)]
struct Watch {
    definition: WatchDefinition,
    samples: u64,
    baseline: Option<Baseline>,
}

#[derive(Debug, Default)]
pub struct WatchStore {
    watches: BTreeMap<String, Watch>,
}

impl WatchStore {
    pub fn define(&mut self, definition: WatchDefinition) -> Result<WatchMetadata, String> {
        if self.watches.contains_key(&definition.name) {
            return Err(format!(
                "an ECS watch named {:?} already exists",
                definition.name
            ));
        }
        if self.watches.len() >= MAX_WATCHES {
            return Err(format!("ECS watch limit {MAX_WATCHES} reached"));
        }
        let metadata = WatchMetadata {
            definition: definition.clone(),
            samples: 0,
            last_frame_count: None,
        };
        self.watches.insert(
            definition.name.clone(),
            Watch {
                definition,
                samples: 0,
                baseline: None,
            },
        );
        Ok(metadata)
    }

    pub fn definition(&self, name: &str) -> Result<WatchDefinition, String> {
        self.watches
            .get(name)
            .map(|watch| watch.definition.clone())
            .ok_or_else(|| format!("no ECS watch named {name:?}"))
    }

    pub fn list(&self) -> Vec<WatchMetadata> {
        self.watches
            .values()
            .map(|watch| WatchMetadata {
                definition: watch.definition.clone(),
                samples: watch.samples,
                last_frame_count: watch.baseline.as_ref().map(|baseline| baseline.frame_count),
            })
            .collect()
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.watches.remove(name).is_some()
    }

    pub fn clear(&mut self) {
        self.watches.clear();
    }

    /// Preserve definitions while discarding histories across a rewind.
    pub fn reset_baselines(&mut self) {
        for watch in self.watches.values_mut() {
            watch.samples = 0;
            watch.baseline = None;
        }
    }

    pub fn record(
        &mut self,
        name: &str,
        frame_count: u64,
        snapshot: Value,
    ) -> Result<WatchSample, String> {
        let watch = self
            .watches
            .get_mut(name)
            .ok_or_else(|| format!("no ECS watch named {name:?}"))?;
        let parsed = ParsedSnapshot::from_json(snapshot)?;
        if parsed.world != watch.definition.query.world {
            return Err(format!(
                "ECS watch {name:?} expected world {:?}, inspector returned {:?}",
                watch.definition.query.world, parsed.world
            ));
        }

        let current = Baseline {
            frame_count,
            alive: parsed.alive,
            matched: parsed.matched,
            returned: parsed.returned,
            component_counts: parsed.component_counts.clone(),
            entity_ids: parsed.entity_ids,
            snapshot_truncated: parsed.truncated,
        };
        let delta = delta(
            watch.baseline.as_ref(),
            &current,
            watch.definition.entity_delta_limit,
        );
        watch.samples = watch.samples.saturating_add(1);
        watch.baseline = Some(current);

        Ok(WatchSample {
            watch: name.to_string(),
            definition: watch.definition.clone(),
            budgets: WatchBudgets {
                definitions: MAX_WATCHES,
                entities: watch.definition.query.limit,
                entity_delta_ids: watch.definition.entity_delta_limit,
                projection_cells: console_core::ECS_QUERY_CELL_BUDGET,
                projected_string_bytes: console_core::ECS_QUERY_STRING_BUDGET,
                projected_value_bytes: console_core::ECS_QUERY_STRING_MAX,
            },
            sample_index: watch.samples,
            frame_count,
            alive: parsed.alive,
            capacity: parsed.capacity,
            component_type_count: parsed.component_type_count,
            matched: parsed.matched,
            returned: parsed.returned,
            truncated: parsed.truncated,
            budget_exhausted: parsed.budget_exhausted,
            next_after: parsed.next_after,
            component_counts: parsed.component_counts,
            entities: parsed.entities,
            delta,
        })
    }
}

struct ParsedSnapshot {
    world: String,
    alive: u64,
    capacity: u64,
    component_type_count: u64,
    matched: u64,
    returned: u64,
    truncated: bool,
    budget_exhausted: bool,
    next_after: u64,
    component_counts: BTreeMap<String, u64>,
    entity_ids: BTreeSet<u64>,
    entities: Vec<Value>,
}

impl ParsedSnapshot {
    fn from_json(value: Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "ECS inspector returned a non-object".to_string())?;
        let world = json_string(object, "world")?.to_string();
        let alive = json_u64(object, "alive")?;
        let capacity = json_u64(object, "capacity")?;
        let component_type_count = json_u64(object, "component_type_count")?;
        let matched = json_u64(object, "matched")?;
        let returned = json_u64(object, "returned")?;
        let truncated = json_bool(object, "truncated")?;
        let budget_exhausted = json_bool(object, "budget_exhausted")?;
        let next_after = json_u64(object, "next_after")?;
        let component_counts = object
            .get("component_counts")
            .and_then(Value::as_object)
            .ok_or_else(|| "ECS inspector omitted component_counts".to_string())?
            .iter()
            .map(|(name, value)| {
                value
                    .as_u64()
                    .map(|count| (name.clone(), count))
                    .ok_or_else(|| {
                        format!("ECS inspector returned a non-integer count for {name:?}")
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        let entities = object
            .get("entities")
            .and_then(Value::as_array)
            .ok_or_else(|| "ECS inspector omitted entities".to_string())?
            .clone();
        if returned != entities.len() as u64 {
            return Err(format!(
                "ECS inspector returned count {returned} for {} entities",
                entities.len()
            ));
        }
        let mut entity_ids = BTreeSet::new();
        for entity in &entities {
            let id = entity.get("id").and_then(Value::as_u64).ok_or_else(|| {
                "ECS inspector returned an entity without an integer id".to_string()
            })?;
            if !entity_ids.insert(id) {
                return Err(format!("ECS inspector returned duplicate entity id {id}"));
            }
        }
        Ok(Self {
            world,
            alive,
            capacity,
            component_type_count,
            matched,
            returned,
            truncated,
            budget_exhausted,
            next_after,
            component_counts,
            entity_ids,
            entities,
        })
    }
}

fn json_u64(object: &Map<String, Value>, name: &str) -> Result<u64, String> {
    object
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("ECS inspector omitted integer {name}"))
}

fn json_bool(object: &Map<String, Value>, name: &str) -> Result<bool, String> {
    object
        .get(name)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("ECS inspector omitted boolean {name}"))
}

fn json_string<'a>(object: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    object
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("ECS inspector omitted string {name}"))
}

fn signed_delta(current: u64, previous: u64) -> i64 {
    let difference = i128::from(current) - i128::from(previous);
    difference.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

fn delta(previous: Option<&Baseline>, current: &Baseline, limit: usize) -> WatchDelta {
    let Some(previous) = previous else {
        return WatchDelta {
            comparable: false,
            previous_frame_count: None,
            frames: None,
            alive: None,
            matched: None,
            returned: None,
            component_counts: BTreeMap::new(),
            spawned: Vec::new(),
            despawned: Vec::new(),
            spawned_truncated: false,
            despawned_truncated: false,
            entity_membership_complete: !current.snapshot_truncated,
            truncated: current.snapshot_truncated,
        };
    };

    let component_names = previous
        .component_counts
        .keys()
        .chain(current.component_counts.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let component_counts = component_names
        .into_iter()
        .filter_map(|name| {
            let change = signed_delta(
                current.component_counts.get(&name).copied().unwrap_or(0),
                previous.component_counts.get(&name).copied().unwrap_or(0),
            );
            (change != 0).then_some((name, change))
        })
        .collect();

    let spawned_all = current
        .entity_ids
        .difference(&previous.entity_ids)
        .copied()
        .collect::<Vec<_>>();
    let despawned_all = previous
        .entity_ids
        .difference(&current.entity_ids)
        .copied()
        .collect::<Vec<_>>();
    let spawned_truncated = spawned_all.len() > limit;
    let despawned_truncated = despawned_all.len() > limit;
    let entity_membership_complete = !previous.snapshot_truncated && !current.snapshot_truncated;

    WatchDelta {
        comparable: true,
        previous_frame_count: Some(previous.frame_count),
        frames: Some(current.frame_count.saturating_sub(previous.frame_count)),
        alive: Some(signed_delta(current.alive, previous.alive)),
        matched: Some(signed_delta(current.matched, previous.matched)),
        returned: Some(signed_delta(current.returned, previous.returned)),
        component_counts,
        spawned: spawned_all.into_iter().take(limit).collect(),
        despawned: despawned_all.into_iter().take(limit).collect(),
        spawned_truncated,
        despawned_truncated,
        entity_membership_complete,
        truncated: !entity_membership_complete || spawned_truncated || despawned_truncated,
    }
}

pub fn name(value: &str, context: &str) -> Result<String, String> {
    if value.is_empty() || value.len() > 64 {
        return Err(format!("{context} names must be 1-64 bytes"));
    }
    let mut bytes = value.bytes();
    let starts_valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    let rest_valid =
        bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'));
    if !starts_valid || !rest_valid {
        return Err(format!(
            "{context} names must begin with a letter/_ and contain only letters, digits, _, ., or -"
        ));
    }
    Ok(value.to_string())
}

fn component_array(
    params: &Value,
    field: &str,
    maximum: usize,
    context: &str,
) -> Result<Vec<String>, String> {
    let Some(value) = params.get(field) else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items = value
        .as_array()
        .ok_or_else(|| format!("{context} {field:?} must be an array"))?;
    if items.len() > maximum {
        return Err(format!(
            "{context} {field:?} accepts at most {maximum} names"
        ));
    }
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let raw = item
            .as_str()
            .ok_or_else(|| format!("{context} {field:?} must contain only strings"))?;
        let component = name(raw, &format!("{context} {field:?}"))?;
        if result.contains(&component) {
            return Err(format!(
                "{context} {field:?} contains duplicate component {component:?}"
            ));
        }
        result.push(component);
    }
    Ok(result)
}

fn component_selection(
    params: &Value,
    context: &str,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let Some(value) = params.get("select") else {
        return Ok(BTreeMap::new());
    };
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let object = value
        .as_object()
        .ok_or_else(|| format!("{context} \"select\" must be an object"))?;
    if object.len() > console_core::ECS_QUERY_MAX_SELECT {
        return Err(format!(
            "{context} \"select\" accepts at most {} components",
            console_core::ECS_QUERY_MAX_SELECT
        ));
    }
    let mut result = BTreeMap::new();
    for (component, value) in object {
        let component = name(component, &format!("{context} select"))?;
        let fields = value.as_array().ok_or_else(|| {
            format!("{context} select component {component:?} must be an array of field names")
        })?;
        if fields.len() > console_core::ECS_QUERY_MAX_FIELDS {
            return Err(format!(
                "{context} select component {component:?} accepts at most {} fields",
                console_core::ECS_QUERY_MAX_FIELDS
            ));
        }
        let mut selected = Vec::with_capacity(fields.len());
        for field in fields {
            let raw = field.as_str().ok_or_else(|| {
                format!("{context} select {component:?} must contain only strings")
            })?;
            let field = name(raw, &format!("{context} select {component:?}"))?;
            if selected.contains(&field) {
                return Err(format!(
                    "{context} select {component:?} contains duplicate field {field:?}"
                ));
            }
            selected.push(field);
        }
        result.insert(component, selected);
    }
    Ok(result)
}

pub fn parse_query(params: &Value, context: &str) -> Result<QueryDefinition, String> {
    let raw_world = params
        .get("world")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires a \"world\" string param"))?;
    let world = name(raw_world, &format!("{context} world"))?;
    let required = component_array(params, "with", console_core::ECS_QUERY_MAX_WITH, context)?;
    let select = component_selection(params, context)?;
    let limit = match params.get("limit") {
        None | Some(Value::Null) => console_core::ECS_QUERY_DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| (1..=console_core::ECS_QUERY_MAX_LIMIT).contains(value))
            .ok_or_else(|| {
                format!(
                    "{context} \"limit\" must be an integer in 1..={}",
                    console_core::ECS_QUERY_MAX_LIMIT
                )
            })?,
    };
    Ok(QueryDefinition {
        world,
        required,
        select,
        limit,
    })
}

pub fn parse_after(params: &Value, context: &str) -> Result<u64, String> {
    match params.get("after") {
        None | Some(Value::Null) => Ok(0),
        Some(value) => value
            .as_u64()
            .filter(|value| *value <= console_core::ECS_MAX_SAFE_ID)
            .ok_or_else(|| {
                format!(
                    "{context} \"after\" must be an integer in 0..={}",
                    console_core::ECS_MAX_SAFE_ID
                )
            }),
    }
}

pub fn parse_entity_delta_limit(params: &Value, context: &str) -> Result<usize, String> {
    match params.get("entity_delta_limit") {
        None | Some(Value::Null) => Ok(DEFAULT_ENTITY_DELTA_LIMIT),
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value <= MAX_ENTITY_DELTA_LIMIT)
            .ok_or_else(|| {
                format!(
                    "{context} \"entity_delta_limit\" must be an integer in 0..={MAX_ENTITY_DELTA_LIMIT}"
                )
            }),
    }
}

pub fn parse_definition(
    params: &Value,
    name_field: &str,
    context: &str,
) -> Result<WatchDefinition, String> {
    if !params.is_object() {
        return Err(format!("{context} must be a JSON object"));
    }
    let allowed = [
        name_field,
        "world",
        "with",
        "select",
        "limit",
        "entity_delta_limit",
    ];
    for field in params.as_object().expect("checked above").keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(format!(
                "{context} does not accept {field:?}; watches track a bounded stable first page"
            ));
        }
    }
    let raw_name = params
        .get(name_field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{context} requires a {name_field:?} string"))?;
    Ok(WatchDefinition {
        name: name(raw_name, &format!("{context} watch"))?,
        query: parse_query(params, context)?,
        entity_delta_limit: parse_entity_delta_limit(params, context)?,
    })
}

pub fn budgets() -> Value {
    json!({
        "definitions": MAX_WATCHES,
        "query_entities": console_core::ECS_QUERY_MAX_LIMIT,
        "entity_delta_ids": MAX_ENTITY_DELTA_LIMIT,
        "projection_cells": console_core::ECS_QUERY_CELL_BUDGET,
        "projected_string_bytes": console_core::ECS_QUERY_STRING_BUDGET,
        "projected_value_bytes": console_core::ECS_QUERY_STRING_MAX,
    })
}
