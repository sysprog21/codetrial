//! Provider usage observations: token counts by kind and by modality.

use serde_json::Value;

/// Modality counts reported by the provider; absent details are not zero usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModalityUsage {
    pub text: u64,
    pub audio: u64,
    pub image: u64,
    pub video: u64,
    pub other: u64,
    pub samples: u64,
}

impl ModalityUsage {
    pub(super) fn from_details(details: Option<&Value>) -> Self {
        let mut usage = Self::default();
        if let Some(details) = details.and_then(Value::as_array) {
            usage.samples = 1;
            for detail in details {
                let count = match detail.get("tokenCount") {
                    None => 0,
                    Some(count) => match count.as_u64() {
                        Some(count) => count,
                        None => return Self::default(),
                    },
                };
                let target = match detail.get("modality").and_then(Value::as_str) {
                    Some("TEXT") => &mut usage.text,
                    Some("AUDIO") => &mut usage.audio,
                    Some("IMAGE") => &mut usage.image,
                    Some("VIDEO") => &mut usage.video,
                    _ => &mut usage.other,
                };
                *target += count;
            }
        }
        usage
    }

    fn add(&mut self, other: Self) {
        self.text += other.text;
        self.audio += other.audio;
        self.image += other.image;
        self.video += other.video;
        self.other += other.other;
        self.samples += other.samples;
    }

    fn log_fields(&self, direction: &str) -> String {
        format!(
            "{direction}_detail_samples={} {direction}_text_tokens={} {direction}_audio_tokens={} {direction}_image_tokens={} {direction}_video_tokens={} {direction}_other_tokens={}",
            self.samples, self.text, self.audio, self.image, self.video, self.other
        )
    }
}

/// Provider usage observations, not an invoice. Keep completion markers and
/// totals so observed sums can be checked against turn boundaries and billing;
/// the bytes sent by this server cannot measure retained provider context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u64,
    pub response: u64,
    pub cached: u64,
    pub thoughts: u64,
    pub samples: u64,
    pub total: u64,
    pub tool_use_prompt: u64,
    pub turn_complete_samples: u64,
    pub prompt_details: ModalityUsage,
    pub response_details: ModalityUsage,
    pub tool_use_details: ModalityUsage,
    /// What `cached` is made of. Cached tokens are priced by modality too, so a
    /// cached-token total without this cannot be priced.
    pub cache_details: ModalityUsage,
}

impl TokenUsage {
    /// One observation riding a Live message, which says whether that message
    /// also completed a turn.
    pub(super) fn from_live_metadata(metadata: &Value, turn_complete: bool) -> Self {
        Self {
            turn_complete_samples: u64::from(turn_complete),
            ..Self::from_metadata(metadata)
        }
    }

    pub(super) fn from_metadata(metadata: &Value) -> Self {
        let count = |key: &str| metadata.get(key).and_then(Value::as_u64).unwrap_or(0);
        Self {
            prompt: count("promptTokenCount"),
            response: metadata
                .get("responseTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| count("candidatesTokenCount")),
            cached: count("cachedContentTokenCount"),
            thoughts: count("thoughtsTokenCount"),
            samples: 1,
            total: count("totalTokenCount"),
            tool_use_prompt: count("toolUsePromptTokenCount"),
            turn_complete_samples: 0,
            prompt_details: ModalityUsage::from_details(metadata.get("promptTokensDetails")),
            tool_use_details: ModalityUsage::from_details(
                metadata.get("toolUsePromptTokensDetails"),
            ),
            response_details: ModalityUsage::from_details(
                metadata
                    .get("responseTokensDetails")
                    .or_else(|| metadata.get("candidatesTokensDetails")),
            ),
            cache_details: ModalityUsage::from_details(metadata.get("cacheTokensDetails")),
        }
    }

    pub fn add(&mut self, other: Self) {
        self.prompt += other.prompt;
        self.response += other.response;
        self.cached += other.cached;
        self.thoughts += other.thoughts;
        self.samples += other.samples;
        self.total += other.total;
        self.tool_use_prompt += other.tool_use_prompt;
        self.turn_complete_samples += other.turn_complete_samples;
        self.prompt_details.add(other.prompt_details);
        self.response_details.add(other.response_details);
        self.tool_use_details.add(other.tool_use_details);
        self.cache_details.add(other.cache_details);
    }

    /// The fields of a log line, in one spelling for the Live session's total
    /// and each HTTP call.
    pub fn log_fields(&self) -> String {
        format!(
            "prompt_tokens={} response_tokens={} cached_tokens={} thought_tokens={} usage_samples={} total_tokens={} tool_use_prompt_tokens={} turn_complete_samples={} {} {} {} {}",
            self.prompt,
            self.response,
            self.cached,
            self.thoughts,
            self.samples,
            self.total,
            self.tool_use_prompt,
            self.turn_complete_samples,
            self.prompt_details.log_fields("prompt"),
            self.response_details.log_fields("response"),
            self.tool_use_details.log_fields("tool_use"),
            self.cache_details.log_fields("cache")
        )
    }
}
