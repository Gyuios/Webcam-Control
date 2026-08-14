use super::{lerp, to_byte};

#[derive(Clone, Debug, PartialEq)]
pub struct CubeLut {
    size: usize,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
    values: Vec<[f32; 3]>,
}

impl CubeLut {
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut size = None;
        let mut domain_min = [0.0; 3];
        let mut domain_max = [1.0; 3];
        let mut values = Vec::new();
        for (line_index, raw_line) in input.lines().enumerate() {
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() || line.starts_with("TITLE") {
                continue;
            }
            let parts: Vec<_> = line.split_whitespace().collect();
            match parts[0] {
                "LUT_3D_SIZE" => {
                    if parts.len() != 2 {
                        return Err(format!("Invalid LUT_3D_SIZE on line {}", line_index + 1));
                    }
                    let parsed = parts[1]
                        .parse::<usize>()
                        .map_err(|_| format!("Invalid integer on line {}", line_index + 1))?;
                    if !(2..=65).contains(&parsed) {
                        return Err("LUT_3D_SIZE must be between 2 and 65".into());
                    }
                    size = Some(parsed);
                }
                "DOMAIN_MIN" => domain_min = parse_triplet(&parts, line_index)?,
                "DOMAIN_MAX" => domain_max = parse_triplet(&parts, line_index)?,
                "LUT_1D_SIZE" => return Err("1D LUT files are not supported".into()),
                _ => values.push(parse_triplet(&parts, line_index)?),
            }
        }
        let size = size.ok_or("The .cube file has no LUT_3D_SIZE")?;
        let expected = size.checked_pow(3).ok_or("The .cube dimensions overflow")?;
        if values.len() != expected {
            return Err(format!(
                "The .cube file contains {} entries; expected {expected}",
                values.len()
            ));
        }
        if (0..3).any(|axis| domain_max[axis] <= domain_min[axis]) {
            return Err("The .cube domain maximum must exceed its minimum".into());
        }
        Ok(Self {
            size,
            domain_min,
            domain_max,
            values,
        })
    }

    pub fn apply_bgra(&self, frame: &mut [u8], strength: f32) {
        let strength = strength.clamp(0.0, 1.0);
        for pixel in frame.chunks_exact_mut(4) {
            let source = [
                pixel[2] as f32 / 255.0,
                pixel[1] as f32 / 255.0,
                pixel[0] as f32 / 255.0,
            ];
            let mapped = self.sample(source);
            pixel[2] = to_byte(lerp(source[0], mapped[0], strength));
            pixel[1] = to_byte(lerp(source[1], mapped[1], strength));
            pixel[0] = to_byte(lerp(source[2], mapped[2], strength));
        }
    }

    fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let mut base = [0_usize; 3];
        let mut fraction = [0.0_f32; 3];
        for axis in 0..3 {
            let normalized = ((rgb[axis] - self.domain_min[axis])
                / (self.domain_max[axis] - self.domain_min[axis]))
                .clamp(0.0, 1.0)
                * (self.size - 1) as f32;
            base[axis] = normalized.floor() as usize;
            fraction[axis] = normalized - base[axis] as f32;
            base[axis] = base[axis].min(self.size - 2);
        }
        let mut result = [0.0; 3];
        for dz in 0..=1 {
            for dy in 0..=1 {
                for dx in 0..=1 {
                    let weight = axis_weight(dx, fraction[0])
                        * axis_weight(dy, fraction[1])
                        * axis_weight(dz, fraction[2]);
                    let value = self.values[self.index(base[0] + dx, base[1] + dy, base[2] + dz)];
                    for channel in 0..3 {
                        result[channel] += value[channel] * weight;
                    }
                }
            }
        }
        result
    }

    fn index(&self, red: usize, green: usize, blue: usize) -> usize {
        red + green * self.size + blue * self.size * self.size
    }
}

fn parse_triplet(parts: &[&str], line: usize) -> Result<[f32; 3], String> {
    if parts.len() != 3 && parts.len() != 4 {
        return Err(format!("Expected three values on line {}", line + 1));
    }
    let offset = usize::from(parts.len() == 4);
    let mut result = [0.0; 3];
    for index in 0..3 {
        result[index] = parts[index + offset]
            .parse::<f32>()
            .map_err(|_| format!("Invalid number on line {}", line + 1))?;
        if !result[index].is_finite() {
            return Err(format!("Non-finite number on line {}", line + 1));
        }
    }
    Ok(result)
}

fn axis_weight(offset: usize, fraction: f32) -> f32 {
    if offset == 0 {
        1.0 - fraction
    } else {
        fraction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_cube_preserves_color() {
        let lut = CubeLut::parse(
            "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
        )
        .unwrap();
        let mut frame = vec![64, 128, 192, 255];
        lut.apply_bgra(&mut frame, 1.0);
        assert_eq!(frame, vec![64, 128, 192, 255]);
    }
}
