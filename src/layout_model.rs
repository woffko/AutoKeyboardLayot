//! Optional character-level layout model.
//!
//! The model scores the same physical keys read on every layout it knows and
//! returns a probability per layout. It never sees more than the current word
//! and the languages (not the text) of up to four previous words. Training,
//! export and evaluation tools live in `tools/layout_model`; the format is
//! described in `tools/layout_model/export.py` and `docs/layout-model.md`.

use crate::Language;
use std::sync::{Arc, OnceLock};

const MAGIC: &[u8; 4] = b"AKLM";
const VERSION: u16 = 2;
const MAX_LAYOUTS: usize = 16;
const MAX_BUCKETS: u32 = 1 << 20;
const MAX_DIM: usize = 64;
const MAX_HIDDEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutModelError {
    Format,
    Unsupported,
}

/// Features of one layout reading of the typed keys.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub text: &'a str,
    pub in_dictionary: bool,
    pub in_short_list: bool,
}

#[derive(Debug)]
pub struct LayoutModel {
    languages: Vec<Language>,
    buckets: u32,
    dim: usize,
    hidden: usize,
    max_n: usize,
    max_ngrams: usize,
    context: usize,
    scales: Vec<f32>,
    embeddings: Vec<i8>,
    hidden_weight: Vec<f32>,
    hidden_bias: Vec<f32>,
    output_weight: Vec<f32>,
    output_bias: f32,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], LayoutModelError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(LayoutModelError::Format)?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or(LayoutModelError::Format)?;
        self.offset = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, LayoutModelError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, LayoutModelError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, LayoutModelError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, LayoutModelError> {
        let value = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        value
            .is_finite()
            .then_some(value)
            .ok_or(LayoutModelError::Format)
    }
    fn f32s(&mut self, count: usize) -> Result<Vec<f32>, LayoutModelError> {
        (0..count).map(|_| self.f32()).collect()
    }
}

impl LayoutModel {
    pub fn parse(bytes: &[u8]) -> Result<Self, LayoutModelError> {
        let mut reader = Reader { bytes, offset: 0 };
        if reader.take(4)? != MAGIC {
            return Err(LayoutModelError::Format);
        }
        if reader.u16()? != VERSION {
            return Err(LayoutModelError::Unsupported);
        }
        let count = usize::from(reader.u8()?);
        if !(2..=MAX_LAYOUTS).contains(&count) {
            return Err(LayoutModelError::Format);
        }
        let mut languages = Vec::with_capacity(count);
        for _ in 0..count {
            let length = usize::from(reader.u8()?);
            let id =
                std::str::from_utf8(reader.take(length)?).map_err(|_| LayoutModelError::Format)?;
            let language = Language::from_id(id).ok_or(LayoutModelError::Format)?;
            if languages.contains(&language) {
                return Err(LayoutModelError::Format);
            }
            languages.push(language);
        }
        let buckets = reader.u32()?;
        let dim = usize::from(reader.u16()?);
        let hidden = usize::from(reader.u16()?);
        let max_n = usize::from(reader.u8()?);
        let max_ngrams = usize::from(reader.u8()?);
        let context = usize::from(reader.u8()?);
        if buckets == 0
            || buckets > MAX_BUCKETS
            || !(1..=MAX_DIM).contains(&dim)
            || !(1..=MAX_HIDDEN).contains(&hidden)
            || !(1..=8).contains(&max_n)
            || max_ngrams == 0
            || context > 16
        {
            return Err(LayoutModelError::Format);
        }
        let rows = buckets as usize;
        let mut scales = Vec::with_capacity(rows);
        let mut embeddings = Vec::with_capacity(rows * dim);
        for _ in 0..rows {
            scales.push(reader.f32()?);
            embeddings.extend(reader.take(dim)?.iter().map(|&byte| byte as i8));
        }
        let width = dim + dense_width(count);
        let hidden_weight = reader.f32s(hidden * width)?;
        let hidden_bias = reader.f32s(hidden)?;
        let output_weight = reader.f32s(hidden)?;
        let output_bias = reader.f32()?;
        if reader.offset != bytes.len() {
            return Err(LayoutModelError::Format);
        }
        Ok(Self {
            languages,
            buckets,
            dim,
            hidden,
            max_n,
            max_ngrams,
            context,
            scales,
            embeddings,
            hidden_weight,
            hidden_bias,
            output_weight,
            output_bias,
        })
    }

    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    pub fn index_of(&self, language: Language) -> Option<usize> {
        self.languages.iter().position(|&known| known == language)
    }

    /// Probability per model layout. `candidates[i]` is the typed keys read on
    /// `languages()[i]`, or None when that layout cannot produce them or is
    /// not enabled. Returns None when the slice length does not match or the
    /// typed layout has no candidate.
    pub fn probabilities(
        &self,
        candidates: &[Option<Candidate<'_>>],
        typed: usize,
        previous: &[Language],
    ) -> Option<Vec<f32>> {
        let count = self.languages.len();
        if candidates.len() != count {
            return None;
        }
        let typed_text = candidates.get(typed)?.as_ref()?.text;
        let mut context = vec![0.0f32; count];
        let start = previous.len().saturating_sub(self.context);
        for (age, language) in previous[start..].iter().rev().enumerate() {
            if let Some(index) = self.index_of(*language) {
                context[index] += 0.5f32.powi(age as i32);
            }
        }
        let mut logits = vec![f32::NEG_INFINITY; count];
        let width = self.dim + dense_width(count);
        let mut features = vec![0.0f32; width];
        let mut hidden = vec![0.0f32; self.hidden];
        for (index, candidate) in candidates.iter().enumerate() {
            let Some(candidate) = candidate else {
                continue;
            };
            features.fill(0.0);
            let ids = self.ngram_ids(index, candidate.text);
            if !ids.is_empty() {
                for &id in &ids {
                    let row = id as usize;
                    let scale = self.scales[row];
                    let values = &self.embeddings[row * self.dim..(row + 1) * self.dim];
                    for (slot, &value) in features.iter_mut().zip(values) {
                        *slot += f32::from(value) * scale;
                    }
                }
                let used = ids.len() as f32;
                for slot in &mut features[..self.dim] {
                    *slot /= used;
                }
            }
            // Order must match tools/layout_model/train.py dense_features.
            let dense = &mut features[self.dim..];
            dense[0] = f32::from(u8::from(candidate.in_dictionary));
            dense[1] = f32::from(u8::from(candidate.in_short_list));
            dense[2] = f32::from(u8::from(index == typed));
            dense[3] = f32::from(u8::from(candidate.text == typed_text));
            dense[4] = context[index];
            dense[5 + index] = 1.0;
            dense[5 + count..].copy_from_slice(&context);
            for (unit, slot) in hidden.iter_mut().enumerate() {
                let weights = &self.hidden_weight[unit * width..(unit + 1) * width];
                let sum: f32 = weights.iter().zip(&features).map(|(w, x)| w * x).sum();
                *slot = (sum + self.hidden_bias[unit]).max(0.0);
            }
            logits[index] = hidden
                .iter()
                .zip(&self.output_weight)
                .map(|(h, w)| h * w)
                .sum::<f32>()
                + self.output_bias;
        }
        let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut probabilities = vec![0.0f32; count];
        let mut total = 0.0;
        for (probability, &logit) in probabilities.iter_mut().zip(&logits) {
            if logit.is_finite() {
                *probability = (logit - max).exp();
                total += *probability;
            }
        }
        for probability in &mut probabilities {
            *probability /= total;
        }
        Some(probabilities)
    }

    fn ngram_ids(&self, layout: usize, text: &str) -> Vec<u32> {
        let padded: Vec<char> = std::iter::once('<')
            .chain(text.chars().flat_map(char::to_lowercase))
            .chain(std::iter::once('>'))
            .collect();
        let mut ids = Vec::new();
        let mut buffer = Vec::with_capacity(4 * self.max_n + 1);
        'outer: for n in 1..=self.max_n {
            for window in padded.windows(n) {
                if n == 1 && (window[0] == '<' || window[0] == '>') {
                    continue;
                }
                if ids.len() == self.max_ngrams {
                    break 'outer;
                }
                buffer.clear();
                buffer.push(layout as u8 + 1);
                for character in window {
                    let mut encoded = [0; 4];
                    buffer.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
                }
                ids.push(crc32(&buffer) % self.buckets);
            }
        }
        ids
    }
}

/// Dense features after the embedding: dictionary, short list, typed layout,
/// equals typed text, own context weight, layout one-hot and all context.
const fn dense_width(layouts: usize) -> usize {
    5 + 2 * layouts
}

/// The EN/RU/ET model shipped with the agent, parsed once on first use.
pub fn embedded() -> Option<Arc<LayoutModel>> {
    static MODEL: OnceLock<Option<Arc<LayoutModel>>> = OnceLock::new();
    MODEL
        .get_or_init(|| {
            LayoutModel::parse(include_bytes!("../data/layout-model/en-ru-et.aklm"))
                .ok()
                .map(Arc::new)
        })
        .clone()
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

/// CRC-32 (IEEE), identical to zlib.crc32 used by the training tools.
fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &byte| {
        CRC_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn embedded_model_matches_reference_probabilities() {
        let model = embedded().expect("embedded model parses");
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/layout-model-fixture.json"))
                .unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let texts: Vec<Option<String>> = model
                .languages()
                .iter()
                .map(|language| {
                    fixture["candidates"][language.id()]
                        .as_str()
                        .map(str::to_owned)
                })
                .collect();
            let candidates: Vec<Option<Candidate<'_>>> = texts
                .iter()
                .enumerate()
                .map(|(index, text)| {
                    let text = text.as_deref()?;
                    let flags = &fixture["flags"][model.languages()[index].id()];
                    Some(Candidate {
                        text,
                        in_dictionary: flags[0].as_bool().unwrap(),
                        in_short_list: flags[1].as_bool().unwrap(),
                    })
                })
                .collect();
            let typed = model
                .index_of(Language::from_id(fixture["typed"].as_str().unwrap()).unwrap())
                .unwrap();
            let previous: Vec<Language> = fixture["context"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| Language::from_id(id.as_str().unwrap()).unwrap())
                .collect();
            let probabilities = model.probabilities(&candidates, typed, &previous).unwrap();
            for (actual, expected) in probabilities
                .iter()
                .zip(fixture["probabilities"].as_array().unwrap())
            {
                let expected = expected.as_f64().unwrap() as f32;
                assert!(
                    (actual - expected).abs() < 1e-4,
                    "{fixture}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn malformed_models_are_rejected() {
        let bytes = include_bytes!("../data/layout-model/en-ru-et.aklm");
        assert!(LayoutModel::parse(&bytes[..bytes.len() - 1]).is_err());
        let mut extra = bytes.to_vec();
        extra.push(0);
        assert!(LayoutModel::parse(&extra).is_err());
        let mut magic = bytes.to_vec();
        magic[0] = b'X';
        assert_eq!(
            LayoutModel::parse(&magic).unwrap_err(),
            LayoutModelError::Format
        );
    }
}
