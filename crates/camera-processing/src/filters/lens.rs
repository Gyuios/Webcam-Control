use super::bilinear_sample;

pub fn apply(
    frame: &mut [u8],
    width: usize,
    height: usize,
    coefficients: [f32; 6],
    scratch: &mut Vec<u8>,
) {
    let [k1, k2, k3, p1, p2, scale] = coefficients;
    scratch.resize(frame.len(), 0);
    scratch.copy_from_slice(frame);
    let aspect = width as f32 / height.max(1) as f32;
    let zoom = 1.0 + scale;
    for output_y in 0..height {
        for output_x in 0..width {
            let x = ((output_x as f32 + 0.5) / width as f32 * 2.0 - 1.0) * aspect / zoom;
            let y = ((output_y as f32 + 0.5) / height as f32 * 2.0 - 1.0) / zoom;
            let r2 = x * x + y * y;
            let radial = 1.0 + k1 * r2 + k2 * r2 * r2 + k3 * r2 * r2 * r2;
            let distorted_x = x * radial + 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x);
            let distorted_y = y * radial + p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y;
            let source_x = ((distorted_x / aspect + 1.0) * 0.5 * width as f32) - 0.5;
            let source_y = ((distorted_y + 1.0) * 0.5 * height as f32) - 0.5;
            let target = (output_y * width + output_x) * 4;
            if source_x < 0.0
                || source_y < 0.0
                || source_x > (width - 1) as f32
                || source_y > (height - 1) as f32
            {
                frame[target..target + 4].copy_from_slice(&[0, 0, 0, 255]);
            } else {
                bilinear_sample(
                    scratch,
                    width,
                    height,
                    source_x,
                    source_y,
                    &mut frame[target..target + 4],
                );
            }
        }
    }
}
