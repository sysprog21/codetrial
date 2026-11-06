//! The way into a room: joining it as the agent, telling the candidate from
//! the agent and other guests, reading their identity and metadata, and the
//! wait for them to arrive before the interview starts.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// Mints the agent token and connects. Split out so `run_room` reads as the
/// interview loop rather than the LiveKit handshake.
pub(super) async fn join_room(
    config: &AgentConfig,
    room_name: &str,
    agent_identity: &str,
    now_seconds: u64,
) -> Result<
    (Room, tokio::sync::mpsc::UnboundedReceiver<RoomEvent>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let token = livekit_token(LivekitTokenInput {
        api_key: &config.livekit_api_key,
        api_secret: &config.livekit_api_secret,
        name: AGENT_NAME,
        identity: agent_identity,
        room: room_name,
        metadata: "{}",
        now_seconds,
        agent: true,
    })?;

    // The board is the only stream this agent is sent, so the room's ceiling on
    // one is the board's. Left at the SDK default it is five gigabytes, which
    // is a header away from a client that is not the browser buffering this
    // process to death before a single chunk is judged.
    let mut options = RoomOptions::default();
    options.data_stream = options
        .data_stream
        .with_max_payload_byte_length(MAX_BOARD_BYTES);
    crate::install_crypto_provider();
    let (room, events) = Room::connect(&config.livekit_url, &token, options).await?;
    eprintln!(
        "joined room={} identity={}",
        room_name,
        room.local_participant().identity()
    );
    Ok((room, events))
}

fn is_candidate(participant: &RemoteParticipant) -> bool {
    candidate_parts(
        participant.kind(),
        &participant.identity().0,
        &participant.metadata(),
    )
}

/// What `is_candidate` decides, on the three facts it reads, so a test can
/// state the rule without a participant the SDK will not let it build.
pub(super) fn candidate_parts(kind: ParticipantKind, identity: &str, metadata: &str) -> bool {
    kind == ParticipantKind::Standard && candidate_identity_matches(identity, metadata)
}

pub fn candidate_identity_matches(identity: &str, metadata: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(metadata)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("candidateIdentity")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|minted| minted == identity && minted.starts_with("candidate-"))
}

/// Only the candidate this interview bootstrapped from may drive the runtime.
/// Their token grants both `canPublishData` and `canPublish`, so a second
/// participant in the room could otherwise end the interview over the data
/// channel or feed their own microphone to the interviewer over a track.
///
/// An unattributed event is refused too. The SDK reports `None` when the sender
/// is not a remote participant it knows, which for this agent means a
/// server-side publish, and nothing here sends itself packets. Accepting them
/// would buy nothing and leave a hole to argue about.
pub(super) fn is_interview_participant(sender: Option<&str>, candidate_identity: &str) -> bool {
    sender.is_some_and(|sender| sender == candidate_identity)
}

pub(super) fn is_duplicate_agent(participant: &RemoteParticipant, local_identity: &str) -> bool {
    duplicate_agent_parts(
        participant.kind(),
        &participant.identity().to_string(),
        local_identity,
    )
}

/// Another agent in the room, whatever it is called: this one's own identity
/// never counts as its duplicate.
pub(super) fn duplicate_agent_parts(
    kind: ParticipantKind,
    identity: &str,
    local_identity: &str,
) -> bool {
    kind == ParticipantKind::Agent && identity != local_identity
}

/// What the interview needs from whoever joined. Assembled in one place because
/// the candidate can be found two ways, already in the room or arriving, and
/// the pair must not be built differently depending on which.
fn candidate_pair(participant: &RemoteParticipant) -> (String, String) {
    (participant.identity().to_string(), participant.metadata())
}

/// `Ok(None)` when nobody arrived within [`CANDIDATE_JOIN_LIMIT`], or when the
/// page that began the start released it first. A no-show is not a failure,
/// so it does not spend a restart attempt; the agent simply leaves the room
/// rather than holding it open for a candidate who closed the tab between
/// requesting a token and connecting.
/// Events seen while waiting are handed back in `deferred` rather than dropped.
/// A `TrackSubscribed` consumed here used to be gone for good, and the only
/// thing that attaches the candidate's microphone is that event, so losing it
/// left an interview where the agent never heard anything at all.
pub(super) async fn candidate_metadata(
    room: &Room,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    deferred: &mut Vec<RoomEvent>,
    slot: Option<&crate::dispatch::Slot>,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(participant) = room
        .remote_participants()
        .values()
        .find(|participant| is_candidate(participant))
    {
        return Ok(Some(candidate_pair(participant)));
    }
    eprintln!("waiting for candidate in room={}", room.name());
    arriving_candidate(&room.name(), events, deferred, slot).await
}

/// The wait half of [`candidate_metadata`], apart from the `Room` so the
/// dispatcher's tests can drive it with a channel they fill themselves.
pub(crate) async fn arriving_candidate(
    room_name: &str,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    deferred: &mut Vec<RoomEvent>,
    slot: Option<&crate::dispatch::Slot>,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    let deadline = tokio::time::Instant::now() + CANDIDATE_JOIN_LIMIT;
    let released = async {
        match slot {
            Some(slot) => slot.released().await,
            None => std::future::pending().await,
        }
    };
    let mut released = std::pin::pin!(released);
    loop {
        let event = tokio::select! {
            event = tokio::time::timeout_at(deadline, events.recv()) => event,
            () = &mut released => {
                eprintln!("start released before a candidate joined room={room_name}");
                return Ok(None);
            }
        };
        match event {
            Err(_) => return Ok(None),
            Ok(Some(RoomEvent::ParticipantConnected(participant)))
                if is_candidate(&participant) =>
            {
                return Ok(Some(candidate_pair(&participant)));
            }
            Ok(Some(other)) => {
                // The wait has its own event loop, so a disconnect here never
                // reaches the one in `run_room`. Passing it on in silence is
                // how this ends as the bare "room closed before a candidate
                // joined" below, which names the symptom and not one of the
                // several unrelated causes: a duplicate identity, a deleted
                // room and a dropped signal socket all arrive here looking
                // identical, and they need three different fixes.
                if let RoomEvent::Disconnected { reason } = &other {
                    eprintln!(
                        "disconnected from room={room_name} while waiting for a candidate: {reason:?}"
                    );
                }
                deferred.push(other);
            }
            Ok(None) => return Err("room closed before a candidate joined".into()),
        }
    }
}

pub(super) fn candidate_bootstrap<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    metadata: Option<&str>,
) -> RuntimeBootstrap<'a> {
    let candidate = parse_participant_metadata(metadata);
    let mut boot = crate::runtime::bootstrap_with_rounds(
        config,
        room_name,
        Some(candidate.problem.id),
        candidate.duration_min,
        crate::runtime::RuntimeOptions {
            voice: candidate.voice,
            accent: candidate.accent,
            profile: candidate.profile,
            grounding: candidate.grounding,
            interview_loop: candidate.interview_loop,
            examples_hidden: candidate.examples_hidden,
            example_drawing_enabled: candidate.example_drawing_enabled,
            candidate_led_enabled: candidate.candidate_led_enabled,
            interview_mode: candidate.interview_mode,
            code_execution_disabled: candidate.code_execution_disabled
                && !candidate.interview_mode.is_whiteboard(),
        },
    );
    if let Some(spoken) = candidate
        .language
        .as_deref()
        .and_then(crate::agent::spoken_language)
    {
        boot.greeting = crate::agent::greeting_with_language(spoken);
        boot.language = candidate.language;
    }
    boot
}
