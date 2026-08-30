use crate::agent::{InterviewMode, Problem, build_instructions_for_mode, get_problem, greeting};
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
    pub live_model: &'a str,
    pub report_model: &'a str,
    pub voice: &'a str,
    pub silence_ms: u32,
    pub start_sensitivity: &'a str,
    pub instructions: String,
    pub greeting: String,
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
    let problem = get_problem(problem_id);
    let duration_min = duration_min.clamp(MIN_DURATION_MIN, MAX_DURATION_MIN);

    RuntimeBootstrap {
        room_name,
        problem,
        duration_min,
        mode,
        live_model: &config.gemini_live_model,
        report_model: &config.gemini_report_model,
        voice: &config.gemini_voice,
        silence_ms: config.gemini_silence_ms,
        start_sensitivity: &config.gemini_start_sensitivity,
        instructions: build_instructions_for_mode(problem, duration_min, mode),
        greeting: greeting(),
    }
}

pub fn agent_identity(room_name: &str) -> String {
    format!("interviewer-{room_name}")
}
