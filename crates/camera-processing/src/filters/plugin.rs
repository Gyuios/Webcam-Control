use super::to_byte;
use camera_protocol::{FilterPluginManifest, PluginProcessor};
use std::collections::BTreeMap;

pub fn apply(
    frame: &mut [u8],
    manifest: &FilterPluginManifest,
    parameters: &BTreeMap<String, f32>,
) -> Result<(), String> {
    match &manifest.processor {
        PluginProcessor::ColorMatrix { base, modulations } => {
            let mut matrix = *base;
            for modulation in modulations {
                let descriptor = manifest
                    .parameters
                    .iter()
                    .find(|item| item.id == modulation.parameter)
                    .ok_or_else(|| {
                        "Plugin modulation references an unknown parameter".to_string()
                    })?;
                let value = parameters
                    .get(&descriptor.id)
                    .copied()
                    .unwrap_or(descriptor.default_value);
                matrix[modulation.coefficient as usize] += value * modulation.scale;
            }
            apply_color_matrix(frame, &matrix);
        }
    }
    Ok(())
}

fn apply_color_matrix(frame: &mut [u8], matrix: &[f32; 12]) {
    for pixel in frame.chunks_exact_mut(4) {
        let red = pixel[2] as f32 / 255.0;
        let green = pixel[1] as f32 / 255.0;
        let blue = pixel[0] as f32 / 255.0;
        pixel[2] = to_byte(red * matrix[0] + green * matrix[1] + blue * matrix[2] + matrix[3]);
        pixel[1] = to_byte(red * matrix[4] + green * matrix[5] + blue * matrix[6] + matrix[7]);
        pixel[0] = to_byte(red * matrix[8] + green * matrix[9] + blue * matrix[10] + matrix[11]);
    }
}
