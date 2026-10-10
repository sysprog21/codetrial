use crate::agent::{
    InterviewGrounding, InterviewLoop, InterviewMode, InterviewProfile, Problem,
    build_instructions_for_plan, get_problem, greeting, interview_grounding_json,
    interview_profile_json, sanitize_interview_grounding, sanitize_interview_profile,
};
use crate::config::{AgentConfig, MAX_DURATION_MIN, MIN_DURATION_MIN};

pub const TOPIC_CODE_UPDATE: &str = "code_update";
pub const TOPIC_CONTROL: &str = "control";
pub const TOPIC_INTEGRITY: &str = "integrity";
pub const TOPIC_TEST_RESULTS: &str = "test_results";
pub const TOPIC_REPORT: &str = "report";
pub const TOPIC_TRANSCRIPTION: &str = "lk.transcription";

/// The board image's data stream. A topic of its own rather than a packet on
/// one of the topics above, because a board is tens of kilobytes of JPEG and
/// `publish_data` carries a single packet: it travels as a LiveKit byte
/// stream, which chunks it over the same data channel.
pub const TOPIC_BOARD_IMAGE: &str = "board_image";

pub const TOOL_READ_EDITOR: &str = "read_editor";
pub const TOOL_READ_BOARD: &str = "read_board";
pub const TOOL_LOG_HINT: &str = "log_hint";
pub const TOOL_RECORD_FRAMEWORK_EVIDENCE: &str = "record_framework_evidence";
pub const TOOL_END_INTERVIEW: &str = "end_interview";
pub const AGENT_NAME: &str = "Jim";

/// Everything the Gemini live session needs to open an interview. The LiveKit
/// side is deliberately absent: `run_room` mints its own join token before it
/// knows which problem the candidate picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBootstrap<'a> {
    pub room_name: &'a str,
    pub problem: &'static Problem,
    pub duration_min: u32,
    pub interview_loop: InterviewLoop,
    pub interview_mode: InterviewMode,
    pub code_execution_disabled: bool,
    pub coding_minutes: u32,
    pub behavioral_minutes: u32,
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    pub live_model: &'a str,
    pub report_model: &'a str,
    pub voice: &'a str,
    pub silence_ms: u32,
    pub context_compression: Option<crate::config::GeminiContextCompression>,
    /// Whether candidate video frames go to Gemini, which is what makes the
    /// setup's media resolution worth sending.
    ///
    /// Never at a whiteboard, whatever the operator enabled. The board reaches
    /// Gemini as a realtime image on the same stream a camera frame does, and
    /// `read_board` tells the model to look at the newest one, so a camera
    /// frame every few seconds would replace the board it was just told to
    /// read. The low media resolution the camera asks for would also blur the
    /// handwriting.
    pub candidate_video: bool,
    pub start_sensitivity: &'a str,
    pub end_sensitivity: Option<&'a str>,
    pub instructions: String,
    pub greeting: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeOptions {
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    pub interview_loop: InterviewLoop,
    pub examples_hidden: bool,
    pub interview_mode: InterviewMode,
    pub code_execution_disabled: bool,
}

pub fn bootstrap<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
) -> RuntimeBootstrap<'a> {
    bootstrap_with_rounds(
        config,
        room_name,
        problem_id,
        duration_min,
        RuntimeOptions::default(),
    )
}

pub fn bootstrap_with_rounds<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
    options: RuntimeOptions,
) -> RuntimeBootstrap<'a> {
    let RuntimeOptions {
        profile,
        grounding,
        interview_loop,
        examples_hidden,
        interview_mode,
        code_execution_disabled,
    } = options;
    let problem = get_problem(problem_id);
    let duration_min = duration_min.clamp(MIN_DURATION_MIN, MAX_DURATION_MIN);
    let profile = sanitize_interview_profile(Some(&interview_profile_json(&profile)));
    let grounding = sanitize_interview_grounding(Some(&interview_grounding_json(&grounding)));
    let behavioral_minutes = interview_loop.behavioral_minutes().min(duration_min);
    let coding_minutes = duration_min.saturating_sub(behavioral_minutes);

    RuntimeBootstrap {
        room_name,
        problem,
        duration_min,
        interview_loop,
        interview_mode,
        code_execution_disabled,
        coding_minutes,
        behavioral_minutes,
        instructions: build_instructions_for_plan(
            problem,
            duration_min,
            &RuntimeOptions {
                profile: profile.clone(),
                grounding: grounding.clone(),
                interview_loop,
                examples_hidden,
                interview_mode,
                code_execution_disabled,
            },
        ),
        profile,
        grounding,
        live_model: &config.gemini_live_model,
        report_model: &config.gemini_report_model,
        voice: &config.gemini_voice,
        silence_ms: config.gemini_silence_ms,
        context_compression: config.gemini_context_compression,
        candidate_video: config.gemini_candidate_video_enabled && !interview_mode.is_whiteboard(),
        start_sensitivity: &config.gemini_start_sensitivity,
        end_sensitivity: config.gemini_end_sensitivity.as_deref(),
        greeting: greeting(interview_mode),
    }
}

pub fn agent_identity(room_name: &str) -> String {
    format!("interviewer-{room_name}")
}

pub const TOPIC_TASK_ACTION: &str = "task_action";
pub const TOPIC_TASK_REVISION: &str = "task_revision";
pub const TOPIC_TASK_RUN: &str = "task_run";
pub const TOPIC_TASK_STATE: &str = "task_state";
pub const TOPIC_TASK_REVIEW: &str = "task_review";

pub const TOPIC_TASK_CONNECTED: &str = "task_connected";
pub const TOPIC_TASK_START: &str = "task_start";
pub const TOPIC_TASK_END: &str = "task_end";
pub const TOPIC_TASK_THINKING: &str = "task_thinking";
pub const TOPIC_TASK_ERROR: &str = "task_error";
