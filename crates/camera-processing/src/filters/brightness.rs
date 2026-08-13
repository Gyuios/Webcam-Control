use super::to_byte;

pub fn apply(frame: &mut [u8], amount: f32) {
    for pixel in frame.chunks_exact_mut(4) {
        for channel in pixel.iter_mut().take(3) {
            *channel = to_byte(*channel as f32 / 255.0 + amount);
        }
    }
}
