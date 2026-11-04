use ::livekit::prelude::RemoteAudioTrack;
use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use futures_util::StreamExt as _;
use tokio::sync::{mpsc, watch};

pub(super) const SAMPLE_RATE: usize = 16_000;
pub(super) const CHUNK_SAMPLES: usize = SAMPLE_RATE * 30;

pub(super) struct Chunk {
    pub slot: usize,
    pub elapsed_ms: u64,
    pub samples: Vec<i16>,
}

pub(super) async fn capture(
    track: RemoteAudioTrack,
    slot: usize,
    origin: tokio::time::Instant,
    sender: mpsc::Sender<Chunk>,
    mut stop: watch::Receiver<bool>,
) {
    let mut stream = NativeAudioStream::with_options(
        track.rtc_track(),
        16_000,
        1,
        NativeAudioStreamOptions {
            queue_size_frames: Some(4000),
        },
    );
    let mut samples = Vec::with_capacity(CHUNK_SAMPLES);
    let mut elapsed_ms = 0;
    loop {
        let frame = tokio::select! {
            _=stop.changed()=>None,
            frame=stream.next()=>frame,
        };
        let Some(frame) = frame else { break };
        if samples.is_empty() {
            elapsed_ms = u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX);
        }
        samples.extend_from_slice(&frame.data);
        if chunk_is_full(samples.len()) {
            if sender
                .send(Chunk {
                    slot,
                    elapsed_ms,
                    samples: std::mem::take(&mut samples),
                })
                .await
                .is_err()
            {
                break;
            }
            samples = Vec::with_capacity(CHUNK_SAMPLES);
        }
    }
    stream.close();
    if has_tail(&samples) {
        let _ = sender
            .send(Chunk {
                slot,
                elapsed_ms,
                samples,
            })
            .await;
    }
}

pub(super) fn chunk_is_full(len: usize) -> bool {
    len >= CHUNK_SAMPLES
}

pub(super) fn has_tail(samples: &[i16]) -> bool {
    !samples.is_empty()
}

pub(super) fn wav(samples: &[i16]) -> Vec<u8> {
    let bytes = u32::try_from(samples.len().saturating_mul(2))
        .expect("capture chunks are bounded to thirty seconds");
    let mut out = Vec::with_capacity(samples.len() * 2 + 44);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(bytes + 36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16000u32.to_le_bytes());
    out.extend_from_slice(&32000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

pub(super) fn clip_to_deadline(chunk: &mut Chunk, maximum_ms: u64) {
    let remaining_ms = maximum_ms.saturating_sub(chunk.elapsed_ms);
    let samples = usize::try_from(remaining_ms)
        .unwrap_or(usize::MAX)
        .saturating_mul(SAMPLE_RATE / 1000);
    chunk.samples.truncate(samples);
}
