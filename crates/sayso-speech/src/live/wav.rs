//! The recording of one source: a 16 kHz mono 16-bit WAV file that grows
//! while the session runs. The sizes in the header are right after
//! [`WavWriter::finish`].

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u32 = sayso_protocol::pcm::SAMPLE_RATE;
const HEADER_BYTES: u64 = 44;

/// A sample from -1 to 1 as 16-bit PCM, with the scale of
/// `sayso_protocol::pcm`, so the audio of a `client` source goes back to the
/// same samples that the app sent.
pub(crate) fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0).round().clamp(-32768.0, 32767.0) as i16
}

pub(crate) struct WavWriter {
    path: PathBuf,
    file: BufWriter<File>,
    samples: u64,
}

impl WavWriter {
    /// Make the file, or empty a file that exists. On Unix a new file is for this
    /// user only, because a recording holds what people said.
    pub fn create(path: &Path) -> io::Result<WavWriter> {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = BufWriter::new(options.open(path)?);
        file.write_all(&header(0))?;
        Ok(WavWriter { path: path.to_path_buf(), file, samples: 0 })
    }

    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        let bytes: Vec<u8> = samples.iter().flat_map(|&s| to_i16(s).to_le_bytes()).collect();
        self.file.write_all(&bytes)?;
        self.samples += samples.len() as u64;
        Ok(())
    }

    /// Write the sizes in the header and close the file. Gives the number of samples.
    pub fn finish(mut self) -> io::Result<u64> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&header(self.samples))?;
        self.file.flush()?;
        self.file.get_ref().sync_all()?;
        Ok(self.samples)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn header(samples: u64) -> [u8; HEADER_BYTES as usize] {
    // A WAV file holds at most 4 GiB; that is 37 hours of this audio.
    let data = u32::try_from(samples * 2).unwrap_or(u32::MAX - 36);
    let mut out = [0u8; HEADER_BYTES as usize];
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        out[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(b"RIFF");
    put(&(36 + data).to_le_bytes());
    put(b"WAVEfmt ");
    put(&16u32.to_le_bytes());
    put(&1u16.to_le_bytes()); // PCM
    put(&1u16.to_le_bytes()); // mono
    put(&SAMPLE_RATE.to_le_bytes());
    put(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
    put(&2u16.to_le_bytes()); // bytes per sample
    put(&16u16.to_le_bytes()); // bits per sample
    put(b"data");
    put(&data.to_le_bytes());
    out
}

/// The samples of a 16 kHz mono 16-bit WAV file, from -1 to 1. Only the
/// format that [`WavWriter`] writes.
pub(crate) fn read(path: &Path) -> io::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, format!("{} is not a WAV file of Sayso: {what}", path.display()));
    if bytes.len() < HEADER_BYTES as usize || &bytes[0..4] != b"RIFF" || &bytes[8..16] != b"WAVEfmt " {
        return Err(bad("no RIFF header"));
    }
    if &bytes[36..40] != b"data" {
        return Err(bad("no data chunk after the format"));
    }
    let size = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]) as usize;
    let data = &bytes[HEADER_BYTES as usize..(HEADER_BYTES as usize + size).min(bytes.len())];
    Ok(data.as_chunks::<2>().0.iter().map(|&pair| f32::from(i16::from_le_bytes(pair)) / 32768.0).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_has_the_sizes_after_finish() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mic.wav");
        let mut wav = WavWriter::create(&path).unwrap();
        wav.write(&[0.0, 0.5, -0.5]).unwrap();
        wav.write(&[1.0, -1.0]).unwrap();
        assert_eq!(wav.finish().unwrap(), 5);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 44 + 10);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 36 + 10);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1, "mono");
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16, "16-bit");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 10);
        assert_eq!(read(&path).unwrap(), vec![0.0, 0.5, -0.5, 32767.0 / 32768.0, -1.0]);
    }

    #[test]
    fn client_samples_go_back_to_the_same_pcm() {
        let pcm: Vec<i16> = vec![0, 1, -1, 12345, i16::MAX, i16::MIN];
        let floats = sayso_protocol::pcm::decode_f32(&sayso_protocol::pcm::encode_i16(&pcm)).unwrap();
        assert_eq!(floats.iter().map(|&s| to_i16(s)).collect::<Vec<_>>(), pcm);
    }
}
