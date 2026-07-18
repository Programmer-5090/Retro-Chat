// From NoctaVox by Jaxx497
use spectrum_analyzer::{FrequencyLimit, samples_fft_to_spectrum, windows::hann_window};

pub const NUM_BANDS: usize = 24;

pub struct SpectrumState {
    bins: Vec<f32>,
    decay_factor: f32,
    bands: Vec<(f32, f32)>,
    band_peaks: Vec<f32>,
    sample_rate: u32,
}

impl SpectrumState {
    pub fn update(&mut self, samples: &[f32], sample_rate: u32) {
        if sample_rate == 0 {
            return;
        }

        let fft_size = super::TAP_BUFFER_CAPACITY;

        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            let log_min = 20f32.ln();
            let log_max = 20000f32.ln();
            self.bands = (0..NUM_BANDS)
                .map(|i| {
                    let t0 = (i as f32) / (NUM_BANDS as f32);
                    let t1 = ((i + 1) as f32) / (NUM_BANDS as f32);
                    let f0 = (log_min + t0 * (log_max - log_min)).exp();
                    let f1 = (log_min + t1 * (log_max - log_min)).exp();
                    (f0, f1)
                })
                .collect();
            self.band_peaks.resize(NUM_BANDS, 1e-3);
            self.bins.resize(NUM_BANDS, 0.0);
        }

        if samples.len() < fft_size {
            for bin in self.bins.iter_mut() {
                *bin *= self.decay_factor;
            }
            return;
        }

        let start = samples.len() - fft_size;
        let windowed = hann_window(&samples[start..]);

        let spectrum = match samples_fft_to_spectrum(
            &windowed,
            self.sample_rate,
            FrequencyLimit::Range(20.0, 20000.0),
            None,
        ) {
            Ok(s) => s,
            Err(_) => {
                for bin in self.bins.iter_mut() {
                    *bin *= self.decay_factor;
                }
                return;
            }
        };

        let mut data_iter = spectrum.data().iter().peekable();

        for i in 0..self.bands.len() {
            let (lo, hi) = self.bands[i];
            let mut sum = 0.0_f32;
            let mut count = 0_usize;

            while let Some(&(f, m)) = data_iter.peek() {
                let freq_val = f.val();
                if freq_val < lo {
                    data_iter.next();
                } else if freq_val < hi {
                    sum += m.val();
                    count += 1;
                    data_iter.next();
                } else {
                    break;
                }
            }

            let mag = if count > 0 { sum / count as f32 } else { 0.0 };
            let normalized = mag / (fft_size as f32 / 2.0);

            if normalized > self.band_peaks[i] {
                self.band_peaks[i] = normalized;
            } else {
                self.band_peaks[i] = (self.band_peaks[i] * 0.99).max(1e-3);
            }

            let relative = (normalized / self.band_peaks[i]).clamp(0.0, 1.0);

            if relative > self.bins[i] {
                self.bins[i] = relative;
            } else {
                self.bins[i] *= self.decay_factor;
            }
        }
    }

    pub fn bins(&self) -> &[f32] {
        &self.bins
    }
}

impl Default for SpectrumState {
    fn default() -> Self {
        SpectrumState {
            bins: Vec::new(),
            band_peaks: Vec::new(),
            bands: Vec::new(),
            decay_factor: 0.85,
            sample_rate: 0,
        }
    }
}
