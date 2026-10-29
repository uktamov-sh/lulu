// The beat of what the computer plays, from 16 kHz mono: an onset strength
// every 10 ms (spectral flux, the bass counted thrice), the tempo from its
// autocorrelation over the last 6 s, and a beat clock kept in phase with it.
// Pure computation: no device, no thread, no network.

use std::collections::VecDeque;

use serde::Serialize;

use crate::speech::audio::fft;

const RATE: f64 = 16_000.0;
/// 10 ms between onset measurements: 100 per second.
const HOP: usize = 160;
const FPS: f64 = RATE / HOP as f64;
const FFT: usize = 512;
/// Bins taken into the onset strength: up to 4 kHz (31.25 Hz a bin).
const BINS: usize = 128;
/// Up to ~190 Hz, the kick drum and the bass, counted thrice.
const BASS_BINS: usize = 6;
/// What the tempo is read from, and the least it needs.
const HISTORY: usize = 600;
const MIN_HISTORY: usize = 300;
/// The tempo is measured again twice a second.
const ESTIMATE_EVERY: usize = 50;
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 180.0;
/// Below this autocorrelation (against lag 0) there is no beat to follow.
const MIN_CONFIDENCE: f64 = 0.12;
/// Onsets that hardly vary (a held note's spectral ripple) are no beat either:
/// their spread must reach this (a quiet kick drum's is well over 1).
const MIN_SPREAD: f64 = 0.2;
/// Below about −60 dBFS the computer is silent; a second of it loses the beat.
const SILENCE_RMS: f32 = 1e-3;
const SILENCE_FRAMES: usize = 100;
/// One event every 4 frames (25 a second), and one on each beat.
const EMIT_EVERY: usize = 4;
/// The beat is announced this early, for the capture, the webview and the frame.
pub const LEAD_S: f64 = 0.05;
const LEAD_FRAMES: f64 = LEAD_S * FPS;

/// What the island hears, at most ~30 times a second.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BeatEvent {
    /// Beats per minute; 0 when the beat is lost (one last event says so).
    pub bpm: f32,
    /// How far into the current beat, 0…1.
    pub phase: f32,
    /// How hard the beats hit lately, 0…1.
    pub strength: f32,
    /// A beat starts now.
    pub beat: bool,
}

pub struct BeatDetector {
    pending: Vec<f32>,
    window: Vec<f32>,
    hann: Vec<f64>,
    prev: Vec<f64>,
    onsets: VecDeque<f64>,
    /// Frames since the last estimate, and since the last event.
    since_estimate: usize,
    since_emit: usize,
    /// Frames a beat lasts, while the beat is followed.
    period: Option<f64>,
    /// Beats since the last one, 0…1 (announced `LEAD_S` early).
    phase: f64,
    frames_since_beat: f64,
    strength: f64,
    /// Silent frames in a row.
    quiet: usize,
}

impl Default for BeatDetector {
    fn default() -> Self {
        let hann =
            (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (FFT - 1) as f64).cos()).collect();
        Self {
            pending: Vec::with_capacity(HOP),
            window: vec![0.0; FFT],
            hann,
            prev: vec![0.0; BINS],
            onsets: VecDeque::with_capacity(HISTORY),
            since_estimate: 0,
            since_emit: 0,
            period: None,
            phase: 0.0,
            frames_since_beat: 0.0,
            strength: 0.0,
            quiet: 0,
        }
    }
}

impl BeatDetector {
    /// Feeds 16 kHz samples; `out` gets the events they lead to.
    pub fn feed(&mut self, samples: &[f32], mut out: impl FnMut(BeatEvent)) {
        for &s in samples {
            self.pending.push(s);
            if self.pending.len() == HOP {
                if let Some(event) = self.frame() {
                    out(event);
                }
                self.pending.clear();
            }
        }
    }

    #[cfg(test)]
    pub fn bpm(&self) -> Option<f64> {
        self.period.map(|p| 60.0 * FPS / p)
    }

    /// One hop: measure, re-estimate now and then, advance the beat clock.
    fn frame(&mut self) -> Option<BeatEvent> {
        let rms = (self.pending.iter().map(|s| s * s).sum::<f32>() / HOP as f32).sqrt();
        self.quiet = if rms < SILENCE_RMS { self.quiet + 1 } else { 0 };
        self.window.drain(..HOP);
        self.window.extend_from_slice(&self.pending);
        let onset = if rms < SILENCE_RMS {
            self.prev.fill(0.0);
            0.0
        } else {
            self.onset()
        };
        if self.onsets.len() == HISTORY {
            self.onsets.pop_front();
        }
        self.onsets.push_back(onset);

        let was_following = self.period.is_some();
        self.since_estimate += 1;
        if self.quiet >= SILENCE_FRAMES {
            self.period = None;
        } else if self.since_estimate >= ESTIMATE_EVERY && self.onsets.len() >= MIN_HISTORY {
            self.since_estimate = 0;
            self.estimate();
        }
        let Some(period) = self.period else {
            self.since_emit = 0;
            return was_following.then_some(BeatEvent { bpm: 0.0, phase: 0.0, strength: 0.0, beat: false });
        };

        self.phase += 1.0 / period;
        self.frames_since_beat += 1.0;
        // A beat when the clock comes round, never two within half a beat.
        let mut beat = false;
        if self.phase >= 1.0 {
            self.phase -= self.phase.floor();
            if self.frames_since_beat >= period * 0.5 {
                beat = true;
                self.frames_since_beat = 0.0;
                self.measure_strength(period);
            }
        }
        self.since_emit += 1;
        if !beat && self.since_emit < EMIT_EVERY {
            return None;
        }
        self.since_emit = 0;
        Some(BeatEvent {
            bpm: (60.0 * FPS / period) as f32,
            phase: self.phase as f32,
            strength: self.strength as f32,
            beat,
        })
    }

    /// Spectral flux of the log magnitudes, the bass counted thrice.
    fn onset(&mut self) -> f64 {
        let mut re: Vec<f64> = self.window.iter().zip(&self.hann).map(|(&s, &w)| s as f64 * w).collect();
        let mut im = vec![0.0f64; FFT];
        fft(&mut re, &mut im);
        let mut flux = 0.0;
        for k in 1..BINS {
            let amp = (re[k] * re[k] + im[k] * im[k]).sqrt() * 4.0 / FFT as f64;
            let mag = (1.0 + 1000.0 * amp).ln();
            let rise = (mag - self.prev[k]).max(0.0);
            flux += if k <= BASS_BINS { 3.0 * rise } else { rise };
            self.prev[k] = mag;
        }
        flux
    }

    /// The tempo from the autocorrelation of the onsets (likeliest near 120
    /// BPM, which settles double and half tempos), then where its beats fall.
    fn estimate(&mut self) {
        let x: Vec<f64> = {
            let mean = self.onsets.iter().sum::<f64>() / self.onsets.len() as f64;
            self.onsets.iter().map(|v| v - mean).collect()
        };
        let n = x.len();
        let ac = |lag: usize| -> f64 {
            if lag >= n {
                return 0.0;
            }
            (lag..n).map(|i| x[i] * x[i - lag]).sum::<f64>() / (n - lag) as f64
        };
        let ac0 = ac(0);
        if ac0.sqrt() < MIN_SPREAD {
            self.period = None;
            return;
        }
        let min_lag = (60.0 * FPS / MAX_BPM).floor() as usize;
        let max_lag = (60.0 * FPS / MIN_BPM).ceil() as usize;
        let acs: Vec<f64> = (0..=max_lag + 1).map(ac).collect();
        let score = |lag: usize| -> f64 {
            let bpm = 60.0 * FPS / lag as f64;
            let octaves = (bpm / 120.0).log2();
            let prior = (-0.5 * (octaves / 0.9).powi(2)).exp();
            (acs[lag] + 0.5 * ac(2 * lag)) * prior
        };
        let scores: Vec<(usize, f64)> = (min_lag..=max_lag).map(|l| (l, score(l))).collect();
        let Some(&(best, _)) = scores.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else { return };
        if acs[best] / ac0 < MIN_CONFIDENCE {
            self.period = None;
            return;
        }
        // Between whole frames, from the parabola through the three peaks.
        let (a, b, c) = (acs[best - 1], acs[best], acs[best + 1]);
        let denom = a - 2.0 * b + c;
        let shift = if denom.abs() > 1e-12 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
        let measured = best as f64 + shift;
        let period = match self.period {
            // A small drift is followed smoothly; a new song is taken at once.
            Some(p) if (measured / p - 1.0).abs() < 0.04 => p + (measured - p) * 0.5,
            _ => measured,
        };
        let first = self.period.is_none();
        self.period = Some(period);
        self.align(period, first);
    }

    /// Where the beats fall: the offset whose comb over the last 4 s catches
    /// the most onsets. The clock is set on it the first time, then pulled.
    fn align(&mut self, period: f64, first: bool) {
        let n = self.onsets.len();
        let span = (4.0 * FPS) as usize;
        let at = |back: f64| -> f64 {
            // The onset `back` frames before the newest, between frames.
            let pos = (n - 1) as f64 - back;
            if pos < 0.0 {
                return 0.0;
            }
            let i = pos.floor() as usize;
            let f = pos - i as f64;
            let a = self.onsets[i];
            let b = if i + 1 < n { self.onsets[i + 1] } else { a };
            a + (b - a) * f
        };
        let steps = period.ceil() as usize;
        let mut best = (0.0, f64::MIN);
        for d in 0..steps {
            let mut sum = 0.0;
            let mut back = d as f64;
            while back < span as f64 {
                sum += at(back);
                back += period;
            }
            if sum > best.1 {
                best = (d as f64, sum);
            }
        }
        // The last beat was `best.0` frames ago; announced early by the lead.
        let measured = ((best.0 + LEAD_FRAMES) / period).rem_euclid(1.0);
        if first {
            self.phase = measured;
            self.frames_since_beat = self.phase * period;
            return;
        }
        let mut err = measured - self.phase;
        err -= err.round();
        self.phase = (self.phase + 0.5 * err).rem_euclid(1.0);
    }

    /// How hard the beat that just went by hit, against the onsets around it.
    fn measure_strength(&mut self, period: f64) {
        let n = self.onsets.len();
        let mean = self.onsets.iter().sum::<f64>() / n.max(1) as f64;
        if mean <= 1e-9 {
            return;
        }
        // The last heard beat: one period before the one now announced.
        let centre = (n - 1) as f64 - (period - LEAD_FRAMES);
        let from = (centre - 0.15 * period).max(0.0) as usize;
        let to = ((centre + 0.15 * period).max(0.0) as usize).min(n - 1);
        let peak = (from..=to).map(|i| self.onsets[i]).fold(0.0, f64::max);
        let hit = ((peak / mean - 1.0) / 4.0).clamp(0.0, 1.0);
        self.strength += (hit - self.strength) * 0.4;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kick drum every beat: a falling 60 Hz thump and a click of noise.
    fn kicks(bpm: f64, seconds: f64, start: f64) -> Vec<f32> {
        let len = (seconds * RATE) as usize;
        let mut out = vec![0.0f32; len];
        let beat = 60.0 / bpm;
        let mut seed = 12345u32;
        let mut t = start;
        while t < seconds {
            let at = (t * RATE) as usize;
            for i in 0..(0.15 * RATE) as usize {
                if at + i >= len {
                    break;
                }
                let s = i as f64 / RATE;
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let noise = (seed >> 16) as f64 / 32768.0 - 1.0;
                let thump = (2.0 * std::f64::consts::PI * 60.0 * s).sin() * (-s * 25.0).exp();
                let click = noise * (-s * 200.0).exp() * 0.3;
                out[at + i] += (0.6 * thump + click) as f32;
            }
            t += beat;
        }
        out
    }

    /// Feeds it in 10 ms blocks, as the capture does; the time of each beat.
    fn run(d: &mut BeatDetector, samples: &[f32]) -> (Vec<f64>, Vec<BeatEvent>) {
        let mut beats = Vec::new();
        let mut events = Vec::new();
        let mut fed = 0usize;
        for chunk in samples.chunks(160) {
            fed += chunk.len();
            d.feed(chunk, |e| {
                if e.beat {
                    beats.push(fed as f64 / RATE);
                }
                events.push(e);
            });
        }
        (beats, events)
    }

    fn check_tempo(bpm: f64) {
        let start = 0.123;
        let samples = kicks(bpm, 12.0, start);
        let mut d = BeatDetector::default();
        let (beats, events) = run(&mut d, &samples);
        let found = d.bpm().expect("a tempo");
        assert!((found - bpm).abs() < 1.5, "{bpm} BPM heard as {found}");
        let last = events.last().unwrap();
        assert!((last.bpm as f64 - bpm).abs() < 1.5);
        assert!(last.strength > 0.5, "strength {}", last.strength);

        // The last 4 s: one beat per kick, each `LEAD_S` ahead of it.
        let period = 60.0 / bpm;
        let late: Vec<f64> = beats.into_iter().filter(|&t| t > 8.0).collect();
        let expected = 4.0 / period;
        assert!((late.len() as f64 - expected).abs() <= 1.0, "{} beats, {expected:.1} kicks", late.len());
        for t in late {
            let k = ((t + LEAD_S - start) / period).round();
            let off = t + LEAD_S - (start + k * period);
            assert!(off.abs() < 0.04, "{bpm} BPM: beat {off:+.3} s off the kick");
        }
    }

    #[test]
    fn a_kick_drum_gives_its_tempo_and_its_beats() {
        check_tempo(128.0);
        check_tempo(95.0);
        check_tempo(150.0);
    }

    #[test]
    fn a_quiet_kick_drum_is_still_followed() {
        let samples: Vec<f32> = kicks(110.0, 10.0, 0.0).iter().map(|s| s * 0.05).collect();
        let mut d = BeatDetector::default();
        run(&mut d, &samples);
        let found = d.bpm().expect("a tempo");
        assert!((found - 110.0).abs() < 1.5, "{found}");
    }

    #[test]
    fn events_stay_under_thirty_a_second() {
        let samples = kicks(170.0, 10.0, 0.0);
        let mut d = BeatDetector::default();
        let (_, events) = run(&mut d, &samples);
        assert!(events.len() as f64 / 10.0 <= 30.0, "{} events", events.len());
    }

    #[test]
    fn silence_has_no_beat() {
        let mut d = BeatDetector::default();
        let (beats, events) = run(&mut d, &vec![0.0; 10 * RATE as usize]);
        assert!(beats.is_empty() && events.is_empty());
        assert_eq!(d.bpm(), None);
    }

    #[test]
    fn the_beat_stops_when_the_music_does() {
        let mut samples = kicks(120.0, 8.0, 0.0);
        samples.extend(vec![0.0; 3 * RATE as usize]);
        let mut d = BeatDetector::default();
        let (beats, events) = run(&mut d, &samples);
        assert!(beats.iter().all(|&t| t < 9.0), "a beat in the silence: {beats:?}");
        assert_eq!(events.last().unwrap().bpm, 0.0);
        assert_eq!(d.bpm(), None);
    }

    #[test]
    fn a_steady_tone_has_no_beat() {
        let samples: Vec<f32> = (0..10 * RATE as usize)
            .map(|i| (2.0 * std::f64::consts::PI * 220.0 * i as f64 / RATE).sin() as f32 * 0.3)
            .collect();
        let mut d = BeatDetector::default();
        let (beats, _) = run(&mut d, &samples);
        assert!(beats.is_empty(), "{} beats", beats.len());
    }
}
