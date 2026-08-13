use super::to_byte;

pub fn apply(frame: &mut [u8], amount: f32) {
    for pixel in frame.chunks_exact_mut(4) {
        for channel in pixel.iter_mut().take(3) {
            let value = *channel as f32 / 255.0;
            *channel = to_byte((value - 0.5) * amount + 0.5);
        }
    }
}
