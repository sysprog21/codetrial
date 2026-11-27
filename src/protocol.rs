//! The words the browser and the interviewer agree on: the data-channel topics
//! each message travels under, the tools the model may call, and the
//! interviewer's name. `web/lib.js` carries the matching topic names, and the
//! wire fixtures keep the two agreeing. Below the interviewer's logic and the
//! runtime that assembles it, because both read these.

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
pub const TOOL_READ_CONVERSATION: &str = "read_conversation";
pub const TOOL_READ_BOARD: &str = "read_board";
pub const TOOL_LOG_HINT: &str = "log_hint";
pub const TOOL_RECORD_FRAMEWORK_EVIDENCE: &str = "record_framework_evidence";
pub const TOOL_END_INTERVIEW: &str = "end_interview";
pub const AGENT_NAME: &str = "Jim";
