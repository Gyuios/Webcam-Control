//! Ordered CPU reference graph for CameraTuner software filters.
//!
//! Every effect is isolated in `filters/`. A future GPU backend must preserve
//! this graph's order and use its output as the correctness oracle.

mod filters;
mod scaler;

use camera_protocol::{FilterEffect, FilterGraph, FilterPluginManifest, PluginProcessor};
use std::collections::{BTreeMap, HashMap};

pub use filters::lut::CubeLut;
pub use scaler::resize_bgra;

const MAX_FILTER_NODES: usize = 64;
const MAX_PLUGIN_PARAMETERS: usize = 32;

/// Restricts persisted LUT names to one filesystem-safe path component.
pub fn validate_lut_asset_id(asset_id: &str) -> Result<(), String> {
    validate_identifier("LUT asset", asset_id)
}

pub struct ProcessingPipeline {
    graph: FilterGraph,
    lut_assets: HashMap<String, CubeLut>,
    plugins: HashMap<String, FilterPluginManifest>,
    scratch: Vec<u8>,
}

impl ProcessingPipeline {
    pub fn new(
        graph: FilterGraph,
        lut_assets: BTreeMap<String, String>,
        plugins: Vec<FilterPluginManifest>,
    ) -> Result<Self, String> {
        let plugins = validate_plugin_catalog(plugins)?;
        let lut_assets = parse_lut_assets(lut_assets)?;
        validate_graph(&graph, &lut_assets.keys().cloned().collect(), &plugins)?;
        Ok(Self {
            graph,
            lut_assets,
            plugins,
            scratch: Vec::new(),
        })
    }

    pub fn set_graph(&mut self, graph: FilterGraph) -> Result<(), String> {
        if graph == self.graph {
            return Ok(());
        }
        validate_graph(
            &graph,
            &self.lut_assets.keys().cloned().collect(),
            &self.plugins,
        )?;
        self.graph = graph;
        Ok(())
    }

    pub fn set_lut_asset(&mut self, asset_id: String, cube: Option<String>) -> Result<(), String> {
        validate_lut_asset_id(&asset_id)?;
        match cube {
            Some(cube) => {
                self.lut_assets.insert(asset_id, CubeLut::parse(&cube)?);
            }
            None => {
                self.lut_assets.remove(&asset_id);
            }
        }
        validate_graph(
            &self.graph,
            &self.lut_assets.keys().cloned().collect(),
            &self.plugins,
        )
    }

    pub fn process_bgra(
        &mut self,
        frame: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let expected = filters::frame_len(width, height)?;
        if frame.len() != expected {
            return Err(format!(
                "BGRA frame has {} bytes; expected {expected}",
                frame.len()
            ));
        }

        for node in &self.graph.nodes {
            if !node.enabled {
                continue;
            }
            match &node.effect {
                FilterEffect::Brightness { amount } if *amount != 0.0 => {
                    filters::brightness::apply(frame, *amount)
                }
                FilterEffect::Contrast { amount } if *amount != 1.0 => {
                    filters::contrast::apply(frame, *amount)
                }
                FilterEffect::Saturation { amount } if *amount != 1.0 => {
                    filters::saturation::apply(frame, *amount)
                }
                FilterEffect::Gamma { amount } if *amount != 1.0 => {
                    filters::gamma::apply(frame, *amount)
                }
                FilterEffect::Temperature { amount } if *amount != 0.0 => {
                    filters::temperature::apply(frame, *amount)
                }
                FilterEffect::Tint { amount } if *amount != 0.0 => {
                    filters::tint::apply(frame, *amount)
                }
                FilterEffect::Flip {
                    horizontal,
                    vertical,
                } if *horizontal || *vertical => filters::flip::apply(
                    frame,
                    width as usize,
                    height as usize,
                    *horizontal,
                    *vertical,
                ),
                FilterEffect::LensCorrection {
                    k1,
                    k2,
                    k3,
                    p1,
                    p2,
                    scale,
                } => {
                    let coefficients = [*k1, *k2, *k3, *p1, *p2, *scale];
                    if coefficients.iter().any(|value| *value != 0.0) {
                        filters::lens::apply(
                            frame,
                            width as usize,
                            height as usize,
                            coefficients,
                            &mut self.scratch,
                        );
                    }
                }
                FilterEffect::Lut3d {
                    asset_id: Some(asset_id),
                    strength,
                    ..
                } if *strength > 0.0 => {
                    if let Some(lut) = self.lut_assets.get(asset_id) {
                        lut.apply_bgra(frame, *strength);
                    }
                }
                FilterEffect::Plugin {
                    plugin_id,
                    parameters,
                } => {
                    let manifest = self
                        .plugins
                        .get(plugin_id)
                        .ok_or_else(|| format!("Filter plugin '{plugin_id}' is not installed"))?;
                    filters::plugin::apply(frame, manifest, parameters)?;
                }
                _ => {}
            }
        }
        Ok(())
    }
}

pub fn validate_plugin_manifest(manifest: &FilterPluginManifest) -> Result<(), String> {
    if manifest.schema_version != 1 {
        return Err(format!(
            "Plugin '{}' uses schema {}; expected 1",
            manifest.id, manifest.schema_version
        ));
    }
    validate_identifier("plugin", &manifest.id)?;
    if manifest.name.trim().is_empty() || manifest.name.chars().count() > 80 {
        return Err(format!("Plugin '{}' has an invalid name", manifest.id));
    }
    if manifest.version.trim().is_empty() || manifest.version.chars().count() > 32 {
        return Err(format!("Plugin '{}' has an invalid version", manifest.id));
    }
    if manifest.description.chars().count() > 1024 || manifest.author.chars().count() > 128 {
        return Err(format!(
            "Plugin '{}' has oversized descriptive fields",
            manifest.id
        ));
    }
    if manifest.parameters.len() > MAX_PLUGIN_PARAMETERS {
        return Err(format!(
            "Plugin '{}' exposes more than {MAX_PLUGIN_PARAMETERS} parameters",
            manifest.id
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for parameter in &manifest.parameters {
        validate_identifier("plugin parameter", &parameter.id)?;
        if !seen.insert(parameter.id.as_str()) {
            return Err(format!(
                "Plugin '{}' repeats parameter '{}'",
                manifest.id, parameter.id
            ));
        }
        if parameter.label.trim().is_empty()
            || !parameter.minimum.is_finite()
            || !parameter.maximum.is_finite()
            || !parameter.step.is_finite()
            || !parameter.default_value.is_finite()
            || parameter.minimum.abs() > 1_000_000.0
            || parameter.maximum.abs() > 1_000_000.0
            || parameter.minimum >= parameter.maximum
            || parameter.step <= 0.0
            || !(parameter.minimum..=parameter.maximum).contains(&parameter.default_value)
        {
            return Err(format!(
                "Plugin '{}' has invalid parameter '{}'",
                manifest.id, parameter.id
            ));
        }
    }
    match &manifest.processor {
        PluginProcessor::ColorMatrix { base, modulations } => {
            if base
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 16.0)
            {
                return Err(format!("Plugin '{}' has a non-finite matrix", manifest.id));
            }
            for modulation in modulations {
                if modulation.coefficient >= 12
                    || !modulation.scale.is_finite()
                    || modulation.scale.abs() > 16.0
                    || !seen.contains(modulation.parameter.as_str())
                {
                    return Err(format!(
                        "Plugin '{}' has an invalid matrix modulation",
                        manifest.id
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Validates graph edits without reparsing potentially large `.cube` assets.
/// Asset contents are validated once when loaded; slider/reorder updates only
/// need identifiers and the plugin catalog.
pub fn validate_filter_graph_config(
    graph: &FilterGraph,
    lut_assets: &BTreeMap<String, String>,
    plugins: &[FilterPluginManifest],
) -> Result<(), String> {
    for id in lut_assets.keys() {
        validate_lut_asset_id(id)?;
    }
    let plugins = validate_plugin_catalog(plugins.to_vec())?;
    validate_graph(graph, &lut_assets.keys().cloned().collect(), &plugins)
}

fn validate_plugin_catalog(
    manifests: Vec<FilterPluginManifest>,
) -> Result<HashMap<String, FilterPluginManifest>, String> {
    let mut result = HashMap::new();
    for manifest in manifests {
        validate_plugin_manifest(&manifest)?;
        let id = manifest.id.clone();
        if result.insert(id.clone(), manifest).is_some() {
            return Err(format!("Filter plugin id '{id}' is duplicated"));
        }
    }
    Ok(result)
}

fn parse_lut_assets(assets: BTreeMap<String, String>) -> Result<HashMap<String, CubeLut>, String> {
    assets
        .into_iter()
        .map(|(id, cube)| {
            validate_lut_asset_id(&id)?;
            CubeLut::parse(&cube).map(|lut| (id, lut))
        })
        .collect()
}

fn validate_graph(
    graph: &FilterGraph,
    lut_assets: &std::collections::HashSet<String>,
    plugins: &HashMap<String, FilterPluginManifest>,
) -> Result<(), String> {
    if graph.nodes.len() > MAX_FILTER_NODES {
        return Err(format!(
            "A filter graph may contain at most {MAX_FILTER_NODES} nodes"
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for node in &graph.nodes {
        validate_identifier("filter node", &node.id)?;
        if !ids.insert(node.id.as_str()) {
            return Err(format!("Filter node id '{}' is duplicated", node.id));
        }
        if !node.enabled {
            continue;
        }
        match &node.effect {
            FilterEffect::Brightness { amount }
            | FilterEffect::Temperature { amount }
            | FilterEffect::Tint { amount } => finite_range("amount", *amount, -0.5, 0.5)?,
            FilterEffect::Contrast { amount } | FilterEffect::Saturation { amount } => {
                finite_range("amount", *amount, 0.0, 2.0)?
            }
            FilterEffect::Gamma { amount } => finite_range("amount", *amount, 0.25, 2.5)?,
            FilterEffect::LensCorrection {
                k1,
                k2,
                k3,
                p1,
                p2,
                scale,
            } => {
                finite_range("lens k1", *k1, -0.5, 0.5)?;
                finite_range("lens k2", *k2, -0.25, 0.25)?;
                finite_range("lens k3", *k3, -0.1, 0.1)?;
                finite_range("lens p1", *p1, -0.05, 0.05)?;
                finite_range("lens p2", *p2, -0.05, 0.05)?;
                finite_range("lens scale", *scale, -0.25, 0.5)?;
            }
            FilterEffect::Lut3d {
                asset_id, strength, ..
            } => {
                finite_range("LUT strength", *strength, 0.0, 1.0)?;
                if let Some(asset_id) = asset_id {
                    validate_lut_asset_id(asset_id)?;
                    if !lut_assets.contains(asset_id) {
                        return Err(format!("LUT asset '{asset_id}' is not loaded"));
                    }
                }
            }
            FilterEffect::Plugin {
                plugin_id,
                parameters,
            } => {
                let manifest = plugins
                    .get(plugin_id)
                    .ok_or_else(|| format!("Filter plugin '{plugin_id}' is not installed"))?;
                for descriptor in &manifest.parameters {
                    let value = parameters
                        .get(&descriptor.id)
                        .copied()
                        .unwrap_or(descriptor.default_value);
                    finite_range(
                        &format!("plugin parameter {}", descriptor.id),
                        value,
                        descriptor.minimum,
                        descriptor.maximum,
                    )?;
                }
                if parameters
                    .keys()
                    .any(|id| !manifest.parameters.iter().any(|item| item.id == *id))
                {
                    return Err(format!(
                        "Plugin '{plugin_id}' received an unknown parameter"
                    ));
                }
            }
            FilterEffect::Flip { .. } => {}
        }
    }
    Ok(())
}

fn validate_identifier(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!("Invalid {kind} identifier '{value}'"));
    }
    Ok(())
}

fn finite_range(name: &str, value: f32, minimum: f32, maximum: f32) -> Result<(), String> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(())
    } else {
        Err(format!("{name} must be between {minimum} and {maximum}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camera_protocol::{FilterNode, MatrixModulation, PluginParameter, ScalingMode};

    fn node(id: &str, effect: FilterEffect) -> FilterNode {
        FilterNode {
            id: id.into(),
            enabled: true,
            label: None,
            effect,
        }
    }

    #[test]
    fn empty_graph_is_true_identity() {
        let mut frame = vec![10, 20, 30, 255, 100, 110, 120, 255];
        let expected = frame.clone();
        ProcessingPipeline::new(FilterGraph::default(), BTreeMap::new(), vec![])
            .unwrap()
            .process_bgra(&mut frame, 2, 1)
            .unwrap();
        assert_eq!(frame, expected);
    }

    #[test]
    fn repeated_brightness_nodes_are_applied_in_order() {
        let graph = FilterGraph {
            nodes: vec![
                node("a", FilterEffect::Brightness { amount: 0.1 }),
                node("b", FilterEffect::Brightness { amount: 0.1 }),
            ],
        };
        let mut frame = vec![0, 0, 0, 17];
        ProcessingPipeline::new(graph, BTreeMap::new(), vec![])
            .unwrap()
            .process_bgra(&mut frame, 1, 1)
            .unwrap();
        assert!((50..=52).contains(&frame[0]));
        assert_eq!(frame[3], 17);
    }

    #[test]
    fn hot_graph_update_changes_the_next_frame() {
        let mut pipeline = ProcessingPipeline::new(
            FilterGraph {
                nodes: vec![node("brightness", FilterEffect::Brightness { amount: 0.0 })],
            },
            BTreeMap::new(),
            vec![],
        )
        .unwrap();
        let mut before = vec![32, 32, 32, 255];
        pipeline.process_bgra(&mut before, 1, 1).unwrap();

        pipeline
            .set_graph(FilterGraph {
                nodes: vec![node("brightness", FilterEffect::Brightness { amount: 0.5 })],
            })
            .unwrap();
        let mut after = vec![32, 32, 32, 255];
        pipeline.process_bgra(&mut after, 1, 1).unwrap();

        assert_eq!(before, vec![32, 32, 32, 255]);
        assert!((159..=160).contains(&after[0]));
        assert_eq!(after[3], 255);
    }

    #[test]
    fn graph_order_changes_non_commutative_output() {
        let brightness = node("brightness", FilterEffect::Brightness { amount: 0.2 });
        let contrast = node("contrast", FilterEffect::Contrast { amount: 2.0 });
        let mut first = vec![64, 64, 64, 255];
        let mut second = first.clone();
        ProcessingPipeline::new(
            FilterGraph {
                nodes: vec![brightness.clone(), contrast.clone()],
            },
            BTreeMap::new(),
            vec![],
        )
        .unwrap()
        .process_bgra(&mut first, 1, 1)
        .unwrap();
        ProcessingPipeline::new(
            FilterGraph {
                nodes: vec![contrast, brightness],
            },
            BTreeMap::new(),
            vec![],
        )
        .unwrap()
        .process_bgra(&mut second, 1, 1)
        .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn declarative_plugin_exposes_and_applies_custom_parameter() {
        let plugin = FilterPluginManifest {
            schema_version: 1,
            id: "example.red-gain".into(),
            name: "Red gain".into(),
            version: "1.0.0".into(),
            description: String::new(),
            author: String::new(),
            parameters: vec![PluginParameter {
                id: "gain".into(),
                label: "Gain".into(),
                minimum: 0.0,
                maximum: 2.0,
                step: 0.01,
                default_value: 1.0,
            }],
            processor: PluginProcessor::ColorMatrix {
                base: [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                modulations: vec![MatrixModulation {
                    parameter: "gain".into(),
                    coefficient: 0,
                    scale: 1.0,
                }],
            },
        };
        let graph = FilterGraph {
            nodes: vec![node(
                "plugin",
                FilterEffect::Plugin {
                    plugin_id: plugin.id.clone(),
                    parameters: BTreeMap::from([("gain".into(), 2.0)]),
                },
            )],
        };
        let mut frame = vec![10, 20, 80, 255];
        ProcessingPipeline::new(graph, BTreeMap::new(), vec![plugin])
            .unwrap()
            .process_bgra(&mut frame, 1, 1)
            .unwrap();
        assert_eq!(frame[2], 160);
        assert_eq!(frame[1], 20);
    }

    #[test]
    fn malformed_plugin_is_rejected() {
        let plugin = FilterPluginManifest {
            schema_version: 1,
            id: "bad plugin".into(),
            name: "Bad".into(),
            version: "1".into(),
            description: String::new(),
            author: String::new(),
            parameters: vec![],
            processor: PluginProcessor::ColorMatrix {
                base: [0.0; 12],
                modulations: vec![],
            },
        };
        assert!(validate_plugin_manifest(&plugin).is_err());
    }

    #[test]
    fn lut_asset_ids_cannot_escape_the_asset_directory() {
        assert!(validate_lut_asset_id("lut-filter-1").is_ok());
        assert!(validate_lut_asset_id("../outside").is_err());
        assert!(validate_lut_asset_id("nested/path").is_err());
        assert!(validate_lut_asset_id("").is_err());
    }

    #[test]
    fn ai_scaler_is_explicitly_unavailable_without_backend() {
        assert!(resize_bgra(&[0, 0, 0, 255], 1, 1, 2, 2, ScalingMode::Ai).is_err());
    }
}
