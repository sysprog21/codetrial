//! Bounded stereo PCM: slot zero is left, slot one is right. Capture timestamps
//! align channels; an absent frame stays silent rather than moving the clock.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u64 = 16_000;
pub(crate) const MAX_AUDIO_SECONDS: u64 = crate::config::MAX_CAPTURE_SECONDS;

pub(crate) struct PeerAudioArtifact {
    pub bytes: u64,
    pub duration_ms: u64,
}

pub(crate) struct PeerAudioWriter {
    file: Option<File>,
    partial: PathBuf,
    destination: PathBuf,
    frames: u64,
    max_frames: u64,
    finished: bool,
}

pub(crate) fn audio_path(dir: &Path, id: &str) -> io::Result<PathBuf> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid peer session id",
        ));
    }
    Ok(dir.join(format!("{id}.wav")))
}

pub(crate) fn ensure_private_directory(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "peer audio directory must be private (0700)",
            ));
        }
    }
    Ok(())
}

impl PeerAudioWriter {
    pub(crate) fn begin(dir: &Path, id: &str, seconds: u64) -> io::Result<Self> {
        if seconds == 0 || seconds > MAX_AUDIO_SECONDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer audio duration exceeds its bound",
            ));
        }
        let destination = audio_path(dir, id)?;
        ensure_private_directory(dir)?;
        if destination.try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "peer audio already exists",
            ));
        }
        let partial = destination.with_extension("wav.part");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&partial)?;
        let mut writer = Self {
            file: Some(file),
            partial,
            destination,
            frames: 0,
            max_frames: seconds * SAMPLE_RATE,
            finished: false,
        };
        writer.write_header()?;
        Ok(writer)
    }

    fn file(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "peer audio is closed"))
    }

    pub(crate) fn append(
        &mut self,
        slot: usize,
        elapsed_ms: u64,
        samples: &[i16],
    ) -> io::Result<()> {
        if slot > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid peer audio slot",
            ));
        }
        let start = elapsed_ms
            .checked_mul(SAMPLE_RATE)
            .map(|value| value / 1000)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "peer audio timestamp overflow")
            })?;
        let count = u64::try_from(samples.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "peer audio frame too large")
        })?;

        // The observer collects at most thirty seconds per chunk. Bound one
        // allocation too, so a caller cannot turn a chunk into the full room.
        if count > SAMPLE_RATE * 30 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer audio frame too large",
            ));
        }

        // The final chunk can cross the cutoff while a stop reaches its audio
        // task. Keep its in-window prefix instead of losing the whole file.
        let count = count.min(self.max_frames.saturating_sub(start));
        if count == 0 {
            return Ok(());
        }
        let length = usize::try_from(count)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "peer audio size overflow"))?;
        let samples = &samples[..length];
        let end = start + count;
        let mut interleaved = vec![0_u8; samples.len() * 4];
        let offset = 44 + start * 4;
        let existing = self.frames.saturating_sub(start).min(count);
        let existing_bytes = usize::try_from(existing * 4)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "peer audio size overflow"))?;
        let file = self.file()?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut interleaved[..existing_bytes])?;
        for (index, sample) in samples.iter().enumerate() {
            let at = index * 4 + slot * 2;
            interleaved[at..at + 2].copy_from_slice(&sample.to_le_bytes());
        }
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&interleaved)?;
        self.frames = self.frames.max(end);
        Ok(())
    }

    fn write_header(&mut self) -> io::Result<()> {
        let data_bytes = u32::try_from(self.frames * 4)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "peer WAV too large"))?;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&2_u16.to_le_bytes());
        header.extend_from_slice(&16_000_u32.to_le_bytes());
        header.extend_from_slice(&64_000_u32.to_le_bytes());
        header.extend_from_slice(&4_u16.to_le_bytes());
        header.extend_from_slice(&16_u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&data_bytes.to_le_bytes());
        let file = self.file()?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header)
    }

    pub(crate) fn finish(mut self) -> io::Result<PeerAudioArtifact> {
        self.write_header()?;
        self.file()?.sync_all()?;
        drop(self.file.take());
        fs::rename(&self.partial, &self.destination)?;
        self.finished = true;
        Ok(PeerAudioArtifact {
            bytes: 44 + self.frames * 4,
            duration_ms: self.frames * 1000 / SAMPLE_RATE,
        })
    }
}

impl Drop for PeerAudioWriter {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.finished {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/peer_audio/wav.rs"]
mod tests;
