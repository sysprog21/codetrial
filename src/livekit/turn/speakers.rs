//! The two speakers' open turns, and closing them: one turn each, counted in
//! the evidence ledger and published as a final segment in the order they
//! opened.

use std::time::Instant;

use crate::agent::{CANDIDATE_SPEAKER, EvidenceLedger, INTERVIEWER_SPEAKER, SpeakerTurn};
use crate::livekit::transcripts::Segment;

/// One open turn per speaker. Gemini interleaves input and output
/// transcription, so they cannot share a single accumulator.
#[derive(Debug, Default)]
pub(in crate::livekit) struct SpeakerTurns {
    pub(in crate::livekit) interviewer: SpeakerTurn,
    pub(in crate::livekit) candidate: SpeakerTurn,
    /// The cut waiting for the words behind it; see [`SpeakerTurns::note_cut`].
    cut: Option<WaitingCut>,
}

/// How much of what the candidate said over a cut the line quotes.
const CUT_QUOTE_CHARS: usize = 80;

/// What the line for a cut needs from the moment of the cut, and the words
/// that closes while the candidate was speaking cleared from the turn since.
#[derive(Debug)]
struct WaitingCut {
    at: String,
    since: Instant,
    heard: String,
}

/// The start of two stretches of speech, one after the other.
fn joined_head(earlier: &str, later: &str) -> String {
    let mut words = String::from(earlier);
    if !earlier.is_empty() && !later.is_empty() {
        words.push(' ');
    }
    words.push_str(later);
    words.chars().take(CUT_QUOTE_CHARS).collect()
}

impl SpeakerTurns {
    /// Gemini cut the interviewer's turn, and the line naming the cut is
    /// already out. `Interrupted` arrives with the start of the candidate's
    /// activity, before the transcription of whatever caused it, so the line
    /// quoting the cause waits for the words. Noted after the turn being
    /// replied to has been closed, or that answer is what the line quotes. A
    /// cut still waiting is replaced, so settle it first.
    pub(in crate::livekit) fn note_cut(&mut self, at: String, since: Instant) {
        self.cut = Some(WaitingCut {
            at,
            since,
            heard: String::new(),
        });
    }

    pub(in crate::livekit) fn candidate_is_open(&self) -> bool {
        self.candidate.is_open()
    }

    /// The transcript line the interviewer's open turn began at.
    pub(in crate::livekit) fn interviewer_line(&self) -> Option<usize> {
        self.interviewer.transcript_line()
    }

    /// Adds what the interviewer said to their open turn and returns the
    /// interim segment to publish.
    pub(in crate::livekit) fn record_interviewer(
        &mut self,
        transcript: &mut Vec<String>,
        text: &str,
    ) -> Segment {
        let text = self
            .interviewer
            .record(transcript, INTERVIEWER_SPEAKER, text);
        Segment {
            id: self.interviewer.segment_id("interviewer"),
            text,
            final_segment: false,
            identity: None,
        }
    }

    /// The same for the candidate, attributed to `identity`.
    pub(in crate::livekit) fn record_candidate(
        &mut self,
        transcript: &mut Vec<String>,
        text: &str,
        identity: &str,
    ) -> Segment {
        let text = self.candidate.record(transcript, CANDIDATE_SPEAKER, text);
        Segment {
            id: self.candidate.segment_id("candidate"),
            text,
            final_segment: false,
            identity: Some(identity.to_string()),
        }
    }

    /// Ends only the interviewer's turn, for a line a cut sealed: the
    /// candidate's turn may still be open and must not be split.
    pub(in crate::livekit) fn close_interviewer(
        &mut self,
        ledger: &mut EvidenceLedger,
        receipt_timestamp_ms: u64,
    ) -> Option<Segment> {
        close_counted(
            &mut self.interviewer,
            "interviewer",
            ledger,
            receipt_timestamp_ms,
            None,
        )
    }

    /// The room-free half of `close_turns`: finishes each open turn, counts it
    /// in the ledger, and returns its final segment, in publishing order. Only
    /// the candidate's own speech is attributed to them; the interviewer
    /// publishes as the agent participant.
    pub(in crate::livekit) fn close_both(
        &mut self,
        ledger: &mut EvidenceLedger,
        receipt_timestamp_ms: u64,
        candidate_identity: Option<&str>,
    ) -> Vec<Segment> {
        let order = closing_order(
            self.interviewer.transcript_line(),
            self.candidate.transcript_line(),
        );
        order
            .into_iter()
            .filter_map(|speaker| {
                let (turn, identity) = if speaker == "candidate" {
                    (&mut self.candidate, candidate_identity)
                } else {
                    (&mut self.interviewer, None)
                };
                close_counted(turn, speaker, ledger, receipt_timestamp_ms, identity)
            })
            .collect()
    }

    /// The line for the waiting cut, once, quoting the start of what the
    /// candidate said over it and saying how long after the cut that is. Read
    /// before the candidate turn closes, which clears it.
    ///
    /// Not while the candidate is still speaking: a close that lands
    /// mid-utterance, such as the late end of the cut generation, would read
    /// speech as wordless. The cut keeps waiting, and keeps the words the turn
    /// holds, since that close is about to clear them.
    pub(in crate::livekit) fn take_cut_line(
        &mut self,
        now: Instant,
        speaking: bool,
    ) -> Option<String> {
        let cut = self.cut.as_mut()?;
        let heard = joined_head(&cut.heard, &self.candidate.head(CUT_QUOTE_CHARS));
        if speaking {
            cut.heard = heard;
            return None;
        }
        let cut = self.cut.take()?;
        Some(format!(
            "timing: candidate audio since the cut at={}, settled {:.1}s later: {}",
            cut.at,
            now.saturating_duration_since(cut.since).as_secs_f64(),
            if heard.is_empty() {
                "(nothing transcribed; check speaker echo or background noise)"
            } else {
                &heard
            }
        ))
    }
}

/// Ends one speaker's turn, counts it in the ledger, and returns its final
/// segment, or `None` when the speaker had nothing open.
fn close_counted(
    turn: &mut SpeakerTurn,
    speaker: &str,
    ledger: &mut EvidenceLedger,
    receipt_timestamp_ms: u64,
    identity: Option<&str>,
) -> Option<Segment> {
    let (text, id) = close_turn(turn, speaker)?;
    ledger.record_conversation_turn(receipt_timestamp_ms, speaker);
    Some(Segment {
        id,
        text,
        final_segment: true,
        identity: identity.map(str::to_string),
    })
}

/// Ends the turn and hands back what has to be published as final, or `None`
/// when the speaker had nothing open.
pub(in crate::livekit) fn close_turn(
    turn: &mut SpeakerTurn,
    speaker: &str,
) -> Option<(String, String)> {
    if !turn.is_open() {
        return None;
    }
    let closed = (turn.text(), turn.segment_id(speaker));
    turn.finish();
    Some(closed)
}

/// The two speakers in the order their turns opened.
///
/// Both can be open at once, and whichever is written to the ledger first is a
/// claim about who spoke first. Closing them in a fixed speaker order made that
/// claim out of the source: a candidate answer that finished while a late
/// interviewer fragment was still arriving was recorded after the question it
/// had already answered, and a reader of the ledger saw an answer that preceded
/// nothing. A turn's transcript line is where it opened, so the lower line
/// spoke first. A speaker with no line has nothing to record and sorts last.
pub(in crate::livekit) fn closing_order(
    interviewer_line: Option<usize>,
    candidate_line: Option<usize>,
) -> [&'static str; 2] {
    match (interviewer_line, candidate_line) {
        (Some(interviewer), Some(candidate)) if candidate < interviewer => {
            ["candidate", "interviewer"]
        }
        (None, Some(_)) => ["candidate", "interviewer"],
        _ => ["interviewer", "candidate"],
    }
}
