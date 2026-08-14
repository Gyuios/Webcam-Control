use super::{lerp, to_byte};

pub fn apply(frame: &mut [u8], amount: f32) {
    for pixel in frame.chunks_exact_mut(4) {
        let blue = pixel[0] as f32 / 255.0;
        let green = pixel[1] as f32 / 255.0;
        let red = pixel[2] as f32 / 255.0;
        let luminance = red * 0.2126 + green * 0.7152 + blue * 0.0722;
        pixel[0] = to_byte(lerp(luminance, blue, amount));
        pixel[1] = to_byte(lerp(luminance, green, amount));
        pixel[2] = to_byte(lerp(luminance, red, amount));
    }
}
