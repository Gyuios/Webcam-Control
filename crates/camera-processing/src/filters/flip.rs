pub fn apply(frame: &mut [u8], width: usize, height: usize, horizontal: bool, vertical: bool) {
    if vertical {
        let stride = width * 4;
        for row in 0..height / 2 {
            let opposite = height - 1 - row;
            let split = opposite * stride;
            let (before, after) = frame.split_at_mut(split);
            before[row * stride..(row + 1) * stride].swap_with_slice(&mut after[..stride]);
        }
    }
    if horizontal {
        for row in frame.chunks_exact_mut(width * 4) {
            for column in 0..width / 2 {
                let opposite = width - 1 - column;
                for channel in 0..4 {
                    row.swap(column * 4 + channel, opposite * 4 + channel);
                }
            }
        }
    }
}
