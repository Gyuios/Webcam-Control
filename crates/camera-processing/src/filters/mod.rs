pub mod brightness;
pub mod contrast;
pub mod flip;
pub mod gamma;
pub mod lens;
pub mod lut;
pub mod plugin;
pub mod saturation;
pub mod temperature;
pub mod tint;

pub(crate) fn frame_len(width: u32, height: u32) -> Result<usize, String> {
    width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| "Invalid frame dimensions".into())
}

pub(crate) fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub(crate) fn lerp(start: f32, end: f32, amount: f32) -> f32 {
    start + (end - start) * amount
}

pub(crate) fn bilinear_sample(
    source: &[u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    output: &mut [u8],
) {
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    for channel in 0..4 {
        let top = lerp(
            source[(y0 * width + x0) * 4 + channel] as f32,
            source[(y0 * width + x1) * 4 + channel] as f32,
            fx,
        );
        let bottom = lerp(
            source[(y1 * width + x0) * 4 + channel] as f32,
            source[(y1 * width + x1) * 4 + channel] as f32,
            fx,
        );
        output[channel] = lerp(top, bottom, fy).round().clamp(0.0, 255.0) as u8;
    }
}
