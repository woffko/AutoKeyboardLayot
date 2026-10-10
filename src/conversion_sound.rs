//! The short, quiet sounds played after a conversion or an undo when the user enables them.
//! Each one is synthesized once into a WAV image in memory; nothing is read from disk, written
//! anywhere, or sent anywhere. Playback belongs to the platform adapter.

/// Which sound to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCue {
    /// A word was converted, automatically or by the hotkey.
    Converted,
    /// A conversion was undone.
    Undone,
}

/// Samples per second of the synthesized sounds.
pub const SAMPLE_RATE: u32 = 22_050;

/// The largest sample magnitude, as a fraction of full scale: about -19 dBFS, so the sound stays
/// well below whatever else is playing at the same system volume.
pub const PEAK: f32 = 0.11;

/// One soft tone: a sine with a quieter upper partial, a short linear attack, an exponential
/// decay and a short fade to silence at the end, so it neither clicks nor rings.
struct Tone {
    frequency: f32,
    partial: f32,
    partial_level: f32,
    duration_ms: u32,
    attack_ms: f32,
    decay_ms: f32,
}

impl SoundCue {
    const fn tone(self) -> Tone {
        match self {
            // A high, short tick.
            Self::Converted => Tone {
                frequency: 880.0,
                partial: 1_320.0,
                partial_level: 0.25,
                duration_ms: 70,
                attack_ms: 4.0,
                decay_ms: 18.0,
            },
            // The same shape a fifth lower and a little longer, so the two are told apart.
            Self::Undone => Tone {
                frequency: 587.0,
                partial: 880.0,
                partial_level: 0.2,
                duration_ms: 90,
                attack_ms: 5.0,
                decay_ms: 25.0,
            },
        }
    }
}

/// The 16-bit mono PCM samples of the cue.
pub fn samples(cue: SoundCue) -> Vec<i16> {
    let tone = cue.tone();
    let count = (SAMPLE_RATE * tone.duration_ms / 1_000) as usize;
    let fade = (SAMPLE_RATE as usize * 3) / 1_000;
    let rate = SAMPLE_RATE as f32;
    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let time_ms = index as f32 * 1_000.0 / rate;
        let attack = (time_ms / tone.attack_ms).min(1.0);
        let decay = (-(time_ms - tone.attack_ms).max(0.0) / tone.decay_ms).exp();
        let tail = ((count - index) as f32 / fade as f32).min(1.0);
        let envelope = attack * decay * tail;
        let phase = 2.0 * std::f32::consts::PI * index as f32 / rate;
        let wave = ((phase * tone.frequency).sin()
            + tone.partial_level * (phase * tone.partial).sin())
            / (1.0 + tone.partial_level);
        let value = envelope * wave * PEAK * f32::from(i16::MAX);
        output.push(value.round() as i16);
    }
    output
}

/// A complete WAV image (RIFF, PCM, 16-bit, mono) of the cue, ready to play from memory.
pub fn wav(cue: SoundCue) -> Vec<u8> {
    let samples = samples(cue);
    let data_bytes = (samples.len() * 2) as u32;
    let mut image = Vec::with_capacity(44 + samples.len() * 2);
    image.extend_from_slice(b"RIFF");
    image.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    image.extend_from_slice(b"WAVE");
    image.extend_from_slice(b"fmt ");
    image.extend_from_slice(&16u32.to_le_bytes());
    image.extend_from_slice(&1u16.to_le_bytes()); // PCM
    image.extend_from_slice(&1u16.to_le_bytes()); // mono
    image.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    image.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
    image.extend_from_slice(&2u16.to_le_bytes()); // block alignment
    image.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    image.extend_from_slice(b"data");
    image.extend_from_slice(&data_bytes.to_le_bytes());
    for sample in samples {
        image.extend_from_slice(&sample.to_le_bytes());
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(image: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(image[offset..offset + 4].try_into().unwrap())
    }

    fn u16_at(image: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(image[offset..offset + 2].try_into().unwrap())
    }

    #[test]
    fn every_cue_is_a_valid_pcm_wav_image() {
        for cue in [SoundCue::Converted, SoundCue::Undone] {
            let image = wav(cue);
            assert_eq!(&image[0..4], b"RIFF");
            assert_eq!(u32_at(&image, 4) as usize, image.len() - 8);
            assert_eq!(&image[8..16], b"WAVEfmt ");
            assert_eq!(u32_at(&image, 16), 16);
            assert_eq!(u16_at(&image, 20), 1, "PCM");
            assert_eq!(u16_at(&image, 22), 1, "mono");
            assert_eq!(u32_at(&image, 24), SAMPLE_RATE);
            assert_eq!(u32_at(&image, 28), SAMPLE_RATE * 2);
            assert_eq!(u16_at(&image, 32), 2);
            assert_eq!(u16_at(&image, 34), 16);
            assert_eq!(&image[36..40], b"data");
            assert_eq!(u32_at(&image, 40) as usize, samples(cue).len() * 2);
            assert_eq!(image.len(), 44 + samples(cue).len() * 2);
        }
    }

    #[test]
    fn the_sounds_are_short_quiet_and_start_and_end_in_silence() {
        for (cue, duration_ms) in [(SoundCue::Converted, 70), (SoundCue::Undone, 90)] {
            let samples = samples(cue);
            assert_eq!(samples.len(), (SAMPLE_RATE * duration_ms / 1_000) as usize);
            let peak = samples
                .iter()
                .map(|sample| sample.unsigned_abs())
                .max()
                .unwrap();
            let limit = (PEAK * f32::from(i16::MAX)).ceil() as u16;
            assert!(peak <= limit, "{cue:?}: peak {peak} above {limit}");
            assert!(peak > limit / 2, "{cue:?}: audible at all ({peak})");
            assert_eq!(samples[0], 0, "{cue:?} starts in silence");
            assert!(
                samples.last().unwrap().unsigned_abs() <= 40,
                "{cue:?} ends near silence"
            );
        }
    }

    #[test]
    fn the_two_cues_differ_and_each_is_the_same_every_time() {
        assert_eq!(wav(SoundCue::Converted), wav(SoundCue::Converted));
        assert_ne!(samples(SoundCue::Converted), samples(SoundCue::Undone));
    }
}
