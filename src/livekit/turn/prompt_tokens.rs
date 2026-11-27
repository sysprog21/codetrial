//! The prompt-token counts a context cut is judged from, kept apart from the
//! rest of the room loop's activity: the watch reads usage and completed
//! turns and nothing else.

/// The prompt-token counts a context cut is judged from. Only a heuristic:
/// the provider reports no compression event, so a cut is read off a drop in
/// the prompt count between completed turns, with tool turns, whose totals sum
/// several invocations, kept as spend and upper bounds rather than readings.
#[derive(Debug, Default)]
pub(in crate::livekit) struct PromptTokenWatch {
    /// The latest prompt count observed since the last completed turn, and the
    /// largest since then, the completed turn's own count included. A turn's
    /// usage may arrive on frames other than the one completing it, so the
    /// drop is judged at completion.
    pub(in crate::livekit) latest_prompt_tokens: Option<u64>,
    pub(in crate::livekit) peak_prompt_tokens: u64,
    /// The last completed single-generation count, without observations from
    /// a turn later found to have used tools.
    comparable_prompt_tokens: u64,
    /// A tool turn's largest observed total bounds each invocation's context.
    /// It can prove a decrease but must never seed the comparable peak.
    tool_prompt_tokens: u64,
    /// The last tool turn's total, kept only to judge whether a cut is
    /// plausible: the context grows through tool turns too, and the frozen
    /// comparable count alone would read it as still near the target. Cleared
    /// by the next comparable count.
    tool_prompt_bound: u64,
    pub(in crate::livekit) context_refresh_pending: bool,
    /// A tool-assisted usage sample can sum several model invocations. Its
    /// prompt total is spend, not a context-size reading comparable with a
    /// single-generation turn. Only a total below a prior comparable count
    /// can prove a cut; it never becomes the next comparison's baseline.
    tool_usage_this_turn: bool,
}

impl PromptTokenWatch {
    /// Comparable observations, periodic or not; a zero says nothing about
    /// context. Tool-turn totals remain spend counters and upper bounds only.
    pub(in crate::livekit) fn observe_prompt_tokens(
        &mut self,
        compression: Option<crate::config::GeminiContextCompression>,
        prompt: u64,
    ) {
        if compression.is_none() || prompt == 0 {
            return;
        }
        if self.tool_usage_this_turn {
            self.tool_prompt_tokens = self.tool_prompt_tokens.max(prompt);
            return;
        }
        self.latest_prompt_tokens = Some(prompt);
        self.peak_prompt_tokens = self.peak_prompt_tokens.max(prompt);
    }

    /// Schedules a checkpoint when the turn just completed ran on a context the
    /// provider had cut. The reference is the larger of the last completed
    /// count and any observation since, so usage reported on a frame of its
    /// own still counts. A drop of the threshold is a cut, and so is any drop
    /// from a context that had reached the trigger: new input in the same turn
    /// can make up most of what was cut, but a context only shrinks when the
    /// provider cuts it. For a tool turn, only a total below the previous
    /// completed single-generation count can prove that decrease. A heuristic,
    /// not an API compression event.
    pub(in crate::livekit) fn observe_turn_complete(
        &mut self,
        compression: Option<crate::config::GeminiContextCompression>,
    ) {
        let tool_turn = std::mem::take(&mut self.tool_usage_this_turn);
        let tool_prompt = std::mem::take(&mut self.tool_prompt_tokens);
        if tool_turn {
            self.latest_prompt_tokens = None;
        }
        let Some(compression) = compression else {
            return;
        };
        let (reference, latest) = if tool_turn {
            if tool_prompt == 0 {
                return;
            }
            (self.comparable_prompt_tokens, tool_prompt)
        } else {
            let Some(latest) = self.latest_prompt_tokens.take() else {
                return;
            };
            (self.peak_prompt_tokens, latest)
        };
        let threshold = u64::from(
            compression
                .trigger_tokens
                .saturating_sub(compression.target_tokens),
        )
        .saturating_div(2)
        .clamp(1, 2048);
        let cut_at_trigger =
            reference >= u64::from(compression.trigger_tokens) && latest < reference;
        if reference.saturating_sub(latest) >= threshold || cut_at_trigger {
            self.context_refresh_pending = true;
            if tool_turn {
                // The aggregate proved a drop but supplies no new comparable
                // baseline. Using it would count the following ordinary
                // response's smaller single invocation as another cut.
                self.comparable_prompt_tokens = 0;
                self.peak_prompt_tokens = 0;
            }
        }
        if tool_turn {
            self.tool_prompt_bound = latest;
        } else {
            self.comparable_prompt_tokens = latest;
            self.peak_prompt_tokens = latest;
            self.tool_prompt_bound = 0;
        }
    }

    /// Whether the context could reach the trigger on the next input: nothing
    /// measured yet, a cut already detected, or the last comparable count in
    /// the upper half of the band a cut drops it through. Just after a cut the
    /// context sits near the target, so the continuity a cut needs is not
    /// carried again until it has grown back toward the trigger. A tool turn's
    /// total counts as an upper bound here, so growth through tool turns is not
    /// missed; erring high costs continuity text, erring low costs the cut.
    pub(in crate::livekit) fn context_cut_plausible(
        &self,
        compression: Option<crate::config::GeminiContextCompression>,
    ) -> bool {
        let Some(compression) = compression else {
            return false;
        };
        let measured = self.peak_prompt_tokens;
        let bound = measured.max(self.tool_prompt_bound);
        let trigger = u64::from(compression.trigger_tokens);
        let band = trigger.saturating_sub(u64::from(compression.target_tokens));
        self.context_refresh_pending || measured == 0 || bound >= trigger - band / 2
    }

    pub(in crate::livekit) fn mark_tool_usage(&mut self) {
        if !self.tool_usage_this_turn {
            self.tool_prompt_tokens = self.latest_prompt_tokens.take().unwrap_or(0);

            // Usage can precede the tool-call frame. Retract that provisional
            // peak when the turn proves to contain multiple invocations.
            self.peak_prompt_tokens = self.comparable_prompt_tokens;
        }
        self.tool_usage_this_turn = true;
        self.latest_prompt_tokens = None;
    }

    /// What a replacement keeps: only a resumed socket's baseline, for the
    /// reason `RuntimeActivity::reset_context_observations` gives.
    pub(in crate::livekit) fn reset(&mut self, resumed: bool) {
        let kept = resumed.then_some((
            self.peak_prompt_tokens,
            self.comparable_prompt_tokens,
            self.tool_prompt_bound,
        ));
        *self = Self::default();
        if let Some((peak, comparable, bound)) = kept {
            self.peak_prompt_tokens = peak;
            self.comparable_prompt_tokens = comparable;
            self.tool_prompt_bound = bound;
        }
    }

    /// Whether a checkpoint is owed because the provider cut the context.
    pub(in crate::livekit) fn context_refresh_needed(&self) -> bool {
        self.context_refresh_pending
    }

    /// The owed checkpoint went out.
    pub(in crate::livekit) fn context_refreshed(&mut self) {
        self.context_refresh_pending = false;
    }
}
