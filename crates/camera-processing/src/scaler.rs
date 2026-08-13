use crate::filters::{bilinear_sample, frame_len};
use camera_protocol::ScalingMode;

pub fn resize_bgra(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    mode: ScalingMode,
) -> Result<Vec<u8>, String> {
    if source.len() != frame_len(source_width, source_height)? {
        return Err("Source BGRA bytes do not match their dimensions".into());
    }
    frame_len(target_width, target_height)?;
    if source_width == target_width && source_height == target_height {
        return Ok(source.to_vec());
    }
    match mode {
        ScalingMode::FastBilinear => Ok(resize_bilinear(
            source,
            source_width as usize,
            source_height as usize,
            target_width as usize,
            target_height as usize,
        )),
        ScalingMode::QualityLanczos3 => Ok(resize_lanczos3(
            source,
            source_width as usize,
            source_height as usize,
            target_width as usize,
            target_height as usize,
        )),
        ScalingMode::Ai => Err("AI scaling requires a loaded ONNX backend".into()),
    }
}

fn resize_bilinear(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    let mut target = vec![0; target_width * target_height * 4];
    let scale_x = source_width as f32 / target_width as f32;
    let scale_y = source_height as f32 / target_height as f32;
    for target_y in 0..target_height {
        let source_y =
            ((target_y as f32 + 0.5) * scale_y - 0.5).clamp(0.0, (source_height - 1) as f32);
        for target_x in 0..target_width {
            let source_x =
                ((target_x as f32 + 0.5) * scale_x - 0.5).clamp(0.0, (source_width - 1) as f32);
            let offset = (target_y * target_width + target_x) * 4;
            bilinear_sample(
                source,
                source_width,
                source_height,
                source_x,
                source_y,
                &mut target[offset..offset + 4],
            );
        }
    }
    target
}

fn resize_lanczos3(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    let horizontal_weights = lanczos_weights(source_width, target_width);
    let vertical_weights = lanczos_weights(source_height, target_height);
    let mut horizontal = vec![0.0_f32; target_width * source_height * 4];
    for source_y in 0..source_height {
        for (target_x, weights) in horizontal_weights.iter().enumerate() {
            for channel in 0..4 {
                horizontal[(source_y * target_width + target_x) * 4 + channel] = weights
                    .iter()
                    .map(|(source_x, weight)| {
                        source[(source_y * source_width + source_x) * 4 + channel] as f32 * weight
                    })
                    .sum();
            }
        }
    }
    let mut target = vec![0_u8; target_width * target_height * 4];
    for (target_y, weights) in vertical_weights.iter().enumerate() {
        for target_x in 0..target_width {
            for channel in 0..4 {
                let value: f32 = weights
                    .iter()
                    .map(|(source_y, weight)| {
                        horizontal[(source_y * target_width + target_x) * 4 + channel] * weight
                    })
                    .sum();
                target[(target_y * target_width + target_x) * 4 + channel] =
                    value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    target
}

fn lanczos_weights(source_size: usize, target_size: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = target_size as f32 / source_size as f32;
    let filter_scale = scale.min(1.0);
    let radius = 3.0 / filter_scale;
    (0..target_size)
        .map(|target| {
            let center = (target as f32 + 0.5) / scale - 0.5;
            let first = (center - radius).ceil() as isize;
            let last = (center + radius).floor() as isize;
            let mut weights: Vec<_> = (first..=last)
                .map(|source| {
                    let index = source.clamp(0, source_size as isize - 1) as usize;
                    let weight = lanczos((center - source as f32) * filter_scale, 3.0);
                    (index, weight)
                })
                .filter(|(_, weight)| weight.abs() > f32::EPSILON)
                .collect();
            let total: f32 = weights.iter().map(|(_, weight)| weight).sum();
            if total.abs() > f32::EPSILON {
                for (_, weight) in &mut weights {
                    *weight /= total;
                }
            }
            weights
        })
        .collect()
}

fn lanczos(value: f32, radius: f32) -> f32 {
    let value = value.abs();
    if value < f32::EPSILON {
        1.0
    } else if value >= radius {
        0.0
    } else {
        let pi_value = std::f32::consts::PI * value;
        (pi_value.sin() / pi_value) * ((pi_value / radius).sin() / (pi_value / radius))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_interpolates_and_preserves_shape() {
        let frame = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let resized = resize_bgra(&frame, 2, 1, 3, 1, ScalingMode::FastBilinear).unwrap();
        assert_eq!(resized.len(), 12);
        assert!((120..=135).contains(&resized[4]));
        assert_eq!(resized[7], 255);
    }
}
