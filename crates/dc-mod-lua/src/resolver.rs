use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;

use crate::manifest::{ModDep, ModManifest, SemVer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMod {
    pub manifest: ModManifest,
    pub root_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolverError {
    DuplicateModId(String),
    MissingDependency {
        mod_id: String,
        dep: ModDep,
    },
    VersionMismatch {
        mod_id: String,
        dep: ModDep,
        found: SemVer,
    },
    Conflict {
        mod_a: String,
        mod_b: String,
        reason: String,
    },
    DependencyCycle {
        cycle: Vec<String>,
    },
}

impl fmt::Display for ResolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverError::DuplicateModId(id) => {
                write!(f, "Duplicate mod id '{}'", id)
            }
            ResolverError::MissingDependency { mod_id, dep } => {
                write!(f, "Mod '{}' requires missing dependency '{}'", mod_id, dep)
            }
            ResolverError::VersionMismatch { mod_id, dep, found } => {
                write!(
                    f,
                    "Mod '{}' requires '{}', but found incompatible version '{}'",
                    mod_id, dep, found
                )
            }
            ResolverError::Conflict {
                mod_a,
                mod_b,
                reason,
            } => {
                write!(
                    f,
                    "Mod conflict between '{}' and '{}': {}",
                    mod_a, mod_b, reason
                )
            }
            ResolverError::DependencyCycle { cycle } => {
                write!(f, "Dependency cycle detected: {}", cycle.join(" -> "))
            }
        }
    }
}

impl std::error::Error for ResolverError {}

/// Resolves and topologically sorts a slice of `ModManifest`s.
pub fn resolve_manifests(manifests: &[ModManifest]) -> Result<Vec<ModManifest>, ResolverError> {
    let mut by_id: BTreeMap<String, &ModManifest> = BTreeMap::new();
    for m in manifests {
        if by_id.contains_key(&m.id) {
            return Err(ResolverError::DuplicateModId(m.id.clone()));
        }
        by_id.insert(m.id.clone(), m);
    }

    // 1. Validate conflicts
    for m in manifests {
        for conflict in &m.conflicts {
            if let Some(other) = by_id.get(&conflict.id) {
                if conflict.matches(&other.version) {
                    return Err(ResolverError::Conflict {
                        mod_a: m.id.clone(),
                        mod_b: other.id.clone(),
                        reason: format!(
                            "'{}' declares conflict with '{}' (found {})",
                            m.id, conflict, other.version
                        ),
                    });
                }
            }
        }
    }

    // 2. Validate hard dependencies & build adjacency graph (prereq -> dependents)
    let mut adj: BTreeMap<String, BTreeSetWrapper> = BTreeMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    for m in manifests {
        adj.insert(m.id.clone(), BTreeSetWrapper::new());
        in_degree.insert(m.id.clone(), 0);
    }

    for m in manifests {
        for dep in &m.dependencies {
            let Some(target) = by_id.get(&dep.id) else {
                return Err(ResolverError::MissingDependency {
                    mod_id: m.id.clone(),
                    dep: dep.clone(),
                });
            };
            if !dep.matches(&target.version) {
                return Err(ResolverError::VersionMismatch {
                    mod_id: m.id.clone(),
                    dep: dep.clone(),
                    found: target.version,
                });
            }
            if dep.id == m.id {
                return Err(ResolverError::DependencyCycle {
                    cycle: vec![m.id.clone(), m.id.clone()],
                });
            }
            if adj.get_mut(&dep.id).unwrap().insert(m.id.clone()) {
                *in_degree.get_mut(&m.id).unwrap() += 1;
            }
        }

        for opt_dep in &m.optional_dependencies {
            if let Some(target) = by_id.get(&opt_dep.id) {
                if !opt_dep.matches(&target.version) {
                    return Err(ResolverError::VersionMismatch {
                        mod_id: m.id.clone(),
                        dep: opt_dep.clone(),
                        found: target.version,
                    });
                }
                if opt_dep.id == m.id {
                    return Err(ResolverError::DependencyCycle {
                        cycle: vec![m.id.clone(), m.id.clone()],
                    });
                }
                if adj.get_mut(&opt_dep.id).unwrap().insert(m.id.clone()) {
                    *in_degree.get_mut(&m.id).unwrap() += 1;
                }
            }
        }
    }

    // 3. Kahn's algorithm with deterministic tie-breaking by (priority, id)
    let mut ready: BinaryHeap<Reverse<(i32, String)>> = BinaryHeap::new();
    for m in manifests {
        if in_degree[&m.id] == 0 {
            ready.push(Reverse((m.priority, m.id.clone())));
        }
    }

    let mut ordered = Vec::with_capacity(manifests.len());
    while let Some(Reverse((_, id))) = ready.pop() {
        let m = by_id[&id];
        ordered.push(m.clone());
        if let Some(dependents) = adj.get(&id) {
            for dep_id in &dependents.items {
                let deg = in_degree.get_mut(dep_id).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    let dm = by_id[dep_id];
                    ready.push(Reverse((dm.priority, dm.id.clone())));
                }
            }
        }
    }

    if ordered.len() != manifests.len() {
        let cycle = find_cycle(&by_id, &adj, &in_degree);
        return Err(ResolverError::DependencyCycle { cycle });
    }

    Ok(ordered)
}

/// Resolves and topologically sorts a collection of `DiscoveredMod`s.
pub fn resolve_mods(mods: Vec<DiscoveredMod>) -> Result<Vec<DiscoveredMod>, ResolverError> {
    let manifests: Vec<ModManifest> = mods.iter().map(|m| m.manifest.clone()).collect();
    let ordered_manifests = resolve_manifests(&manifests)?;
    let mut map: HashMap<String, DiscoveredMod> = mods
        .into_iter()
        .map(|m| (m.manifest.id.clone(), m))
        .collect();
    let mut out = Vec::with_capacity(ordered_manifests.len());
    for m in ordered_manifests {
        if let Some(dm) = map.remove(&m.id) {
            out.push(dm);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct BTreeSetWrapper {
    items: Vec<String>,
}

impl BTreeSetWrapper {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    fn insert(&mut self, val: String) -> bool {
        if self.items.contains(&val) {
            false
        } else {
            self.items.push(val);
            self.items.sort();
            true
        }
    }
}

fn find_cycle(
    by_id: &BTreeMap<String, &ModManifest>,
    adj: &BTreeMap<String, BTreeSetWrapper>,
    in_degree: &HashMap<String, usize>,
) -> Vec<String> {
    let remaining: HashSet<String> = in_degree
        .iter()
        .filter(|(_, &deg)| deg > 0)
        .map(|(k, _)| k.clone())
        .collect();

    // Build dependency graph (node -> its prerequisites in `remaining`) so cycle reads A -> B -> A
    // where A depends on B and B depends on A.
    let mut dep_edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &remaining {
        dep_edges.insert(id.clone(), Vec::new());
    }
    for (prereq, dependents) in adj {
        if !remaining.contains(prereq) {
            continue;
        }
        for dep in &dependents.items {
            if remaining.contains(dep) {
                dep_edges.get_mut(dep).unwrap().push(prereq.clone());
            }
        }
    }

    let mut visited = HashSet::new();
    let mut on_stack = Vec::new();
    for start in by_id.keys() {
        if !remaining.contains(start) || visited.contains(start) {
            continue;
        }
        if let Some(cycle) = dfs_cycle(start, &dep_edges, &mut visited, &mut on_stack) {
            return cycle;
        }
    }

    let mut fallback: Vec<String> = remaining.into_iter().collect();
    fallback.sort();
    fallback
}

fn dfs_cycle(
    node: &str,
    edges: &BTreeMap<String, Vec<String>>,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    visited.insert(node.to_string());
    stack.push(node.to_string());

    if let Some(neighbors) = edges.get(node) {
        for next in neighbors {
            if let Some(pos) = stack.iter().position(|x| x == next) {
                let mut cycle: Vec<String> = stack[pos..].to_vec();
                cycle.push(next.clone());
                return Some(cycle);
            }
            if !visited.contains(next) {
                if let Some(c) = dfs_cycle(next, edges, visited, stack) {
                    return Some(c);
                }
            }
        }
    }

    stack.pop();
    None
}
