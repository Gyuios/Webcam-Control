use super::to_byte;

pub fn apply(frame: &mut [u8], amount: f32) {
    for pixel in frame.chunks_exact_mut(4) {
        let blue = pixel[0] as f32 / 255.0 - amount * 0.12;
        let red = pixel[2] as f32 / 255.0 + amount * 0.12;
        pixel[0] = to_byte(blue);
        pixel[2] = to_byte(red);
    }
}
