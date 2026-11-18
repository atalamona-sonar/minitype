//! NVIDIA Parakeet TDT 0.6B v3 on ONNX Runtime.
//!
//! The model comes as three ONNX graphs:
//!   1. `nemo128.onnx`: waveform -> 128-bin log-mel features
//!   2. encoder: features -> one 1024-d vector per 80 ms frame
//!   3. decoder+joint: (frame, previous token, LSTM state) -> token logits
//!      plus "duration" logits saying how many frames to skip next.
//!
//! Decoding is greedy over that token-and-duration (TDT) output.
//! Input/output names are read from each graph rather than hard-coded.

use std::fs;
use std::path::Path;

use ort::environment::Environment;
use ort::session::Session;
use ort::value::{Outlet, Tensor};

/// Safety cap on tokens emitted for a single encoder frame.
const MAX_TOKENS_PER_FRAME: usize = 10;

pub struct Parakeet {
    _env: Environment,
    pre: Session,
    enc: Session,
    dec: Session,
    vocab: Vec<String>,
    blank: usize,
    /// Shape of each decoder LSTM state, with batch = 1.
    state_shape: Vec<usize>,
}

fn session(env: &Environment, path: &Path) -> Result<Session, String> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let builder = Session::builder(env).map_err(|e| e.to_string())?;
    let mut builder = builder.with_intra_threads(threads).map_err(|e| e.to_string())?;
    builder
        .commit_from_file(path)
        .map_err(|e| format!("couldn't load {}: {e}", path.display()))
}

fn names(list: &[Outlet]) -> Vec<String> {
    list.iter().map(|o| o.name().to_string()).collect()
}

impl Parakeet {
    pub fn load(dir: &Path) -> Result<Self, String> {
        let env = ort::init().build().map_err(|e| e.to_string())?;
        let pre = session(&env, &dir.join("nemo128.onnx"))?;
        let enc = session(&env, &dir.join("encoder-model.int8.onnx"))?;
        let dec = session(&env, &dir.join("decoder_joint-model.int8.onnx"))?;

        // vocab.txt: one "token id" pair per line; the blank token is "<blk>".
        let text = fs::read_to_string(dir.join("vocab.txt")).map_err(|e| e.to_string())?;
        let mut vocab = Vec::new();
        for line in text.lines() {
            let Some((tok, id)) = line.rsplit_once(' ') else { continue };
            let id: usize = id.trim().parse().map_err(|_| format!("bad vocab line: {line}"))?;
            if vocab.len() <= id {
                vocab.resize(id + 1, String::new());
            }
            vocab[id] = tok.to_string();
        }
        let blank = vocab.iter().position(|t| t == "<blk>").unwrap_or(vocab.len() - 1);

        // The 4th decoder input is the first LSTM state, e.g. [2, batch, 640].
        let state_shape = match dec.inputs().get(3).map(|o| o.dtype()) {
            Some(ort::value::ValueType::Tensor { shape, .. }) => {
                shape.iter().map(|&d| if d < 1 { 1 } else { d as usize }).collect()
            }
            _ => vec![2, 1, 640],
        };

        eprintln!(
            "[parakeet] pre {:?} -> {:?}; enc {:?} -> {:?}; dec {:?} -> {:?}; state {:?}; vocab {}",
            names(pre.inputs()), names(pre.outputs()),
            names(enc.inputs()), names(enc.outputs()),
            names(dec.inputs()), names(dec.outputs()),
            state_shape, vocab.len()
        );
        Ok(Self { _env: env, pre, enc, dec, vocab, blank, state_shape })
    }

    pub fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let err = |e: ort::Error| e.to_string();
        if samples.is_empty() {
            return Ok(String::new());
        }

        // 1. Features.
        let pre_in = names(self.pre.inputs());
        let pre_out = names(self.pre.outputs());
        let wave = Tensor::from_array(([1usize, samples.len()], samples.to_vec())).map_err(err)?;
        let wave_len = Tensor::from_array(([1usize], vec![samples.len() as i64])).map_err(err)?;
        let out = self
            .pre
            .run(ort::inputs![pre_in[0].as_str() => wave, pre_in[1].as_str() => wave_len])
            .map_err(err)?;
        let (fshape, feats) = out[pre_out[0].as_str()].try_extract_tensor::<f32>().map_err(err)?;
        let (_, flen) = out[pre_out[1].as_str()].try_extract_tensor::<i64>().map_err(err)?;
        let fshape: Vec<usize> = fshape.iter().map(|&d| d as usize).collect();
        let (feats, flen) = (feats.to_vec(), flen[0]);
        drop(out);

        // 2. Encoder. Output is [1, dim, frames].
        let enc_in = names(self.enc.inputs());
        let enc_out = names(self.enc.outputs());
        let feats = Tensor::from_array((fshape, feats)).map_err(err)?;
        let feats_len = Tensor::from_array(([1usize], vec![flen])).map_err(err)?;
        let out = self
            .enc
            .run(ort::inputs![enc_in[0].as_str() => feats, enc_in[1].as_str() => feats_len])
            .map_err(err)?;
        let (eshape, enc) = out[enc_out[0].as_str()].try_extract_tensor::<f32>().map_err(err)?;
        let (_, elen) = out[enc_out[1].as_str()].try_extract_tensor::<i64>().map_err(err)?;
        // `stride` is the full frame count of the [1, dim, frames] layout.
        let (dim, stride) = (eshape[1] as usize, eshape[2] as usize);
        let frames = stride.min(elen[0].max(0) as usize);
        let enc = enc.to_vec();
        drop(out);

        // 3. Greedy TDT decoding.
        let dec_in = names(self.dec.inputs());
        let dec_out = names(self.dec.outputs());
        let state_len: usize = self.state_shape.iter().product();
        let mut state1 = vec![0f32; state_len];
        let mut state2 = vec![0f32; state_len];
        let mut last = self.blank as i32;
        let mut tokens: Vec<usize> = Vec::new();
        let mut t = 0;
        let mut emitted = 0;
        let mut frame = vec![0f32; dim];

        while t < frames {
            for (d, v) in frame.iter_mut().enumerate() {
                *v = enc[d * stride + t];
            }
            let out = self
                .dec
                .run(ort::inputs![
                    dec_in[0].as_str() => Tensor::from_array(([1usize, dim, 1], frame.clone())).map_err(err)?,
                    dec_in[1].as_str() => Tensor::from_array(([1usize, 1], vec![last])).map_err(err)?,
                    dec_in[2].as_str() => Tensor::from_array(([1usize], vec![1i32])).map_err(err)?,
                    dec_in[3].as_str() => Tensor::from_array((self.state_shape.clone(), state1.clone())).map_err(err)?,
                    dec_in[4].as_str() => Tensor::from_array((self.state_shape.clone(), state2.clone())).map_err(err)?,
                ])
                .map_err(err)?;
            let (_, logits) = out[dec_out[0].as_str()].try_extract_tensor::<f32>().map_err(err)?;
            let vocab_n = self.vocab.len();
            let token = argmax(&logits[..vocab_n]);
            let skip = argmax(&logits[vocab_n..]);

            if token != self.blank {
                // Only a real token advances the decoder's state.
                let (_, s1) = out[dec_out[2].as_str()].try_extract_tensor::<f32>().map_err(err)?;
                let (_, s2) = out[dec_out[3].as_str()].try_extract_tensor::<f32>().map_err(err)?;
                state1.copy_from_slice(s1);
                state2.copy_from_slice(s2);
                last = token as i32;
                tokens.push(token);
                emitted += 1;
            }
            if skip > 0 {
                t += skip;
                emitted = 0;
            } else if token == self.blank || emitted >= MAX_TOKENS_PER_FRAME {
                t += 1;
                emitted = 0;
            }
        }

        Ok(detokenize(&tokens, &self.vocab))
    }
}

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .fold((0, f32::MIN), |best, (i, &x)| if x > best.1 { (i, x) } else { best })
        .0
}

/// SentencePiece pieces: "▁" marks the start of a word.
fn detokenize(tokens: &[usize], vocab: &[String]) -> String {
    let text: String = tokens.iter().filter_map(|&t| vocab.get(t)).map(String::as_str).collect();
    text.replace('▁', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_pieces() {
        let vocab: Vec<String> = ["▁Hel", "lo", "▁world", "."].iter().map(|s| s.to_string()).collect();
        assert_eq!(detokenize(&[0, 1, 2, 3], &vocab), "Hello world.");
        assert_eq!(argmax(&[0.1, 0.9, 0.3]), 1);
    }
}
