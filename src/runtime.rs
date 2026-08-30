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

pub const TOOL_READ_EDITOR: &str = "read_editor";
pub const TOOL_LOG_HINT: &str = "log_hint";
pub const TOOL_RECORD_FRAMEWORK_EVIDENCE: &str = "record_framework_evidence";
pub const AGENT_NAME: &str = "Jim";

/// Everything the Gemini live session needs to open an interview. The LiveKit
/// side is deliberately absent: `run_room` mints its own join token before it
/// knows which problem the candidate picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBootstrap<'a> {
    pub room_name: &'a str,
    pub problem: &'static Problem,
    pub duration_min: u32,
    pub mode: InterviewMode,
    pub interview_loop: InterviewLoop,
    pub coding_minutes: u32,
    pub behavioral_minutes: u32,
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    pub live_model: &'a str,
    pub report_model: &'a str,
    pub voice: &'a str,
    pub silence_ms: u32,
    pub start_sensitivity: &'a str,
    pub instructions: String,
    pub greeting: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub mode: InterviewMode,
    pub profile: InterviewProfile,
    pub grounding: InterviewGrounding,
    pub interview_loop: InterviewLoop,
}

pub fn bootstrap<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
) -> RuntimeBootstrap<'a> {
    bootstrap_with_mode(
        config,
        room_name,
        problem_id,
        duration_min,
        InterviewMode::Scored,
    )
}

pub fn bootstrap_with_mode<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
    mode: InterviewMode,
) -> RuntimeBootstrap<'a> {
    bootstrap_with_profile(
        config,
        room_name,
        problem_id,
        duration_min,
        mode,
        InterviewProfile::default(),
    )
}

pub fn bootstrap_with_profile<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
    mode: InterviewMode,
    profile: InterviewProfile,
) -> RuntimeBootstrap<'a> {
    bootstrap_with_context(
        config,
        room_name,
        problem_id,
        duration_min,
        mode,
        profile,
        InterviewGrounding::default(),
    )
}

pub fn bootstrap_with_context<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    problem_id: Option<&str>,
    duration_min: u32,
    mode: InterviewMode,
    profile: InterviewProfile,
    grounding: InterviewGrounding,
) -> RuntimeBootstrap<'a> {
    bootstrap_with_rounds(
        config,
        room_name,
        problem_id,
        duration_min,
        RuntimeOptions {
            mode,
            profile,
            grounding,
            interview_loop: InterviewLoop::CodingBehavioral,
        },
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
        mode,
        profile,
        grounding,
        interview_loop,
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
        mode,
        interview_loop,
        coding_minutes,
        behavioral_minutes,
        instructions: build_instructions_for_plan(
            problem,
            duration_min,
            mode,
            &profile,
            &grounding,
            interview_loop,
        ),
        profile,
        grounding,
        live_model: &config.gemini_live_model,
        report_model: &config.gemini_report_model,
        voice: &config.gemini_voice,
        silence_ms: config.gemini_silence_ms,
        start_sensitivity: &config.gemini_start_sensitivity,
        greeting: greeting(),
    }
}

pub fn agent_identity(room_name: &str) -> String {
    format!("interviewer-{room_name}")
}
