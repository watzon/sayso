//! Find the model files in an extracted archive.
//!
//! The archives of one family do not all use the same file names (for
//! example `encoder.int8.onnx` or `tiny-encoder.onnx`), so the engine looks
//! for name parts and does not hard-code full names.

use crate::spec::Recipe;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The files of an offline (final pass) model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineFiles {
    /// A NeMo transducer (Parakeet TDT).
    Transducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
        tokens: PathBuf,
    },
    /// A NeMo CTC model (Parakeet TDT-CTC).
    NemoCtc {
        model: PathBuf,
        tokens: PathBuf,
    },
    Whisper {
        encoder: PathBuf,
        decoder: PathBuf,
        tokens: PathBuf,
    },
    SenseVoice {
        model: PathBuf,
        tokens: PathBuf,
    },
    /// Moonshine v2: an encoder and a merged decoder.
    MoonshineV2 {
        encoder: PathBuf,
        merged_decoder: PathBuf,
        tokens: PathBuf,
    },
    /// Moonshine v1: four models.
    MoonshineV1 {
        preprocessor: PathBuf,
        encoder: PathBuf,
        uncached_decoder: PathBuf,
        cached_decoder: PathBuf,
        tokens: PathBuf,
    },
}

/// The files of a streaming transducer (the live preview).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineFiles {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
}

/// The model files in one folder, not in subfolders.
struct Listing {
    dir: PathBuf,
    files: Vec<PathBuf>,
}

impl Listing {
    fn read(dir: &Path) -> Result<Listing> {
        let mut files = Vec::new();
        for entry in
            std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                files.push(entry.path());
            }
        }
        files.sort();
        Ok(Listing {
            dir: dir.to_path_buf(),
            files,
        })
    }

    fn name(path: &Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }

    /// The model file (`.onnx` or `.ort`) whose name contains `part`. An int8
    /// file wins over a full precision one, then the shorter name.
    fn find_model(&self, part: &str) -> Option<PathBuf> {
        self.files
            .iter()
            .filter(|p| {
                let name = Self::name(p);
                name.contains(part) && (name.ends_with(".onnx") || name.ends_with(".ort"))
            })
            .min_by_key(|p| {
                let name = Self::name(p);
                (!name.contains("int8"), name.len())
            })
            .cloned()
    }

    fn model(&self, part: &str) -> Result<PathBuf> {
        self.find_model(part)
            .with_context(|| format!("no {part} model file in {}", self.dir.display()))
    }

    fn tokens(&self) -> Result<PathBuf> {
        self.files
            .iter()
            .find(|p| Self::name(p).ends_with("tokens.txt"))
            .cloned()
            .with_context(|| format!("no tokens.txt in {}", self.dir.display()))
    }
}

pub fn offline_files(recipe: Recipe, dir: &Path) -> Result<OfflineFiles> {
    let list = Listing::read(dir)?;
    let tokens = list.tokens()?;
    Ok(match recipe {
        Recipe::ParakeetTdt | Recipe::ParakeetTdtCtc => {
            // Some TDT-CTC archives hold the transducer, others the CTC model.
            match (
                list.find_model("encoder"),
                list.find_model("decoder"),
                list.find_model("joiner"),
            ) {
                (Some(encoder), Some(decoder), Some(joiner)) => OfflineFiles::Transducer {
                    encoder,
                    decoder,
                    joiner,
                    tokens,
                },
                _ => OfflineFiles::NemoCtc {
                    model: list.model("model")?,
                    tokens,
                },
            }
        }
        Recipe::Whisper => OfflineFiles::Whisper {
            encoder: list.model("encoder")?,
            decoder: list.model("decoder")?,
            tokens,
        },
        Recipe::SenseVoice => OfflineFiles::SenseVoice {
            model: list.model("model")?,
            tokens,
        },
        Recipe::Moonshine => match list.find_model("merged") {
            Some(merged_decoder) => OfflineFiles::MoonshineV2 {
                encoder: list.model("encode")?,
                merged_decoder,
                tokens,
            },
            None => OfflineFiles::MoonshineV1 {
                preprocessor: list.model("preprocess")?,
                encoder: list.model("encode")?,
                uncached_decoder: list.model("uncached")?,
                cached_decoder: list.model("cached_decode")?,
                tokens,
            },
        },
        Recipe::ZipformerStreaming => bail!("a streaming model has no final pass"),
    })
}

pub fn online_files(dir: &Path) -> Result<OnlineFiles> {
    let list = Listing::read(dir)?;
    Ok(OnlineFiles {
        encoder: list.model("encoder")?,
        decoder: list.model("decoder")?,
        joiner: list.model("joiner")?,
        tokens: list.tokens()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("test_wavs")).unwrap();
        dir
    }

    fn names(files: &OfflineFiles) -> String {
        format!("{files:?}")
    }

    #[test]
    fn parakeet_prefers_int8_and_falls_back_to_ctc() {
        let dir = folder(&[
            "encoder.onnx",
            "encoder.int8.onnx",
            "decoder.int8.onnx",
            "joiner.int8.onnx",
            "tokens.txt",
        ]);
        let OfflineFiles::Transducer { encoder, .. } =
            offline_files(Recipe::ParakeetTdt, dir.path()).unwrap()
        else {
            panic!("not a transducer");
        };
        assert!(encoder.ends_with("encoder.int8.onnx"));

        let dir = folder(&["model.int8.onnx", "tokens.txt"]);
        let files = offline_files(Recipe::ParakeetTdtCtc, dir.path()).unwrap();
        assert!(
            matches!(files, OfflineFiles::NemoCtc { .. }),
            "{}",
            names(&files)
        );
    }

    #[test]
    fn whisper_sense_voice_and_moonshine() {
        let dir = folder(&[
            "tiny-encoder.onnx",
            "tiny-encoder.int8.onnx",
            "tiny-decoder.onnx",
            "tiny-decoder.int8.onnx",
            "tiny-tokens.txt",
        ]);
        let OfflineFiles::Whisper {
            decoder, tokens, ..
        } = offline_files(Recipe::Whisper, dir.path()).unwrap()
        else {
            panic!("not whisper");
        };
        assert!(decoder.ends_with("tiny-decoder.int8.onnx"));
        assert!(tokens.ends_with("tiny-tokens.txt"));

        let dir = folder(&["model.int8.onnx", "tokens.txt"]);
        assert!(matches!(
            offline_files(Recipe::SenseVoice, dir.path()).unwrap(),
            OfflineFiles::SenseVoice { .. }
        ));

        let dir = folder(&[
            "encoder_model.ort",
            "decoder_model_merged.ort",
            "tokens.txt",
            "LICENSE",
        ]);
        let files = offline_files(Recipe::Moonshine, dir.path()).unwrap();
        assert!(
            matches!(files, OfflineFiles::MoonshineV2 { .. }),
            "{}",
            names(&files)
        );

        let dir = folder(&[
            "preprocess.onnx",
            "encode.int8.onnx",
            "uncached_decode.int8.onnx",
            "cached_decode.int8.onnx",
            "tokens.txt",
        ]);
        let OfflineFiles::MoonshineV1 {
            uncached_decoder,
            cached_decoder,
            ..
        } = offline_files(Recipe::Moonshine, dir.path()).unwrap()
        else {
            panic!("not moonshine v1");
        };
        assert!(uncached_decoder.ends_with("uncached_decode.int8.onnx"));
        assert!(cached_decoder.ends_with("cached_decode.int8.onnx"));
    }

    #[test]
    fn streaming_files_and_missing_files() {
        let dir = folder(&[
            "encoder.onnx",
            "decoder.onnx",
            "joiner.onnx",
            "tokens.txt",
            "README.md",
        ]);
        let files = online_files(dir.path()).unwrap();
        assert!(files.joiner.ends_with("joiner.onnx"));

        let dir = folder(&["encoder.onnx", "tokens.txt"]);
        let error = online_files(dir.path()).unwrap_err().to_string();
        assert!(error.starts_with("no decoder model file"), "{error}");
        let dir = folder(&["model.onnx"]);
        assert!(
            offline_files(Recipe::SenseVoice, dir.path()).is_err(),
            "no tokens"
        );
    }
}
