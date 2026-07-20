#[derive(Debug)]
pub struct LinearResampler {
    ratio: f64,
    cursor: f64,
    input: Vec<f32>,
}

impl LinearResampler {
    pub fn new(input_rate_hz: u32, output_rate_hz: u32) -> Self {
        Self {
            ratio: input_rate_hz.max(1) as f64 / output_rate_hz.max(1) as f64,
            cursor: 0.0,
            input: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.input.extend_from_slice(samples);
    }

    pub fn take_available(&mut self) -> Vec<f32> {
        let mut output = Vec::new();
        while self.cursor + 1.0 < self.input.len() as f64 {
            let left_index = self.cursor.floor() as usize;
            let right_index = left_index + 1;
            let frac = (self.cursor - left_index as f64) as f32;
            let left = self.input[left_index];
            let right = self.input[right_index];
            output.push(left + (right - left) * frac);
            self.cursor += self.ratio;
        }
        let consumed = self.cursor.floor() as usize;
        if consumed > 0 {
            self.input.drain(0..consumed.min(self.input.len()));
            self.cursor -= consumed as f64;
        }
        output
    }
}
