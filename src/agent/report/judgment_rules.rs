//! What a report may not say about a candidate, as data: the claims it is
//! refused for and the phrases that make each one.

/// A class of claim a report may not make about a person, the reason the
/// repair is told, and the phrases that make it. Phrases are named in the
/// error because the lists are ours and the model cannot see them: told only
/// that a check is unsupported, a repair swaps "appeared nervous" for "sounded
/// confident" and fails again. Each phrase is whole words, spelled with the
/// spaces `refused_judgments` pads the text with.
pub(super) struct JudgmentRule {
    pub(super) reason: &'static str,
    /// Only where the report says what to fix, rather than everywhere.
    pub(super) improvements_only: bool,
    pub(super) phrases: &'static [&'static str],
    /// Words that break the rule only when a speech word follows, directly or
    /// through "language", as "a Japanese response" or "a Spanish-language
    /// answer" does.
    pub(super) before_speech: &'static [&'static str],
}

/// What a report calls a candidate's spoken turn. Not "text" or "characters",
/// which a text exercise's input has too.
pub(super) const SPEECH_WORDS: &[&str] = &[
    "response",
    "responses",
    "answer",
    "answers",
    "reply",
    "replies",
    "remark",
    "remarks",
    "statement",
    "statements",
    "explanation",
    "explanations",
    "utterance",
    "utterances",
    "speech",
    "transcript",
    "transcripts",
    "transcription",
    "turn",
    "turns",
];

pub(super) const JUDGMENT_RULES: [JudgmentRule; 3] = [
    JudgmentRule {
        reason: "unsupported delivery or personality judgment",
        improvements_only: false,
        phrases: &[
            " accent ",
            " accents ",
            " dialect ",
            " dialects ",
            " typing speed ",
            " typing pace ",
            " typed slowly ",
            " typed quickly ",
            " type slowly ",
            " type quickly ",
            " speech rate ",
            " filler word ",
            " filler words ",
            " disfluency ",
            " disfluencies ",
            // A disfluency by another name: an accepted success criterion asked
            // for an answer "without hesitation". Not the verb, since "do not
            // hesitate to ask a clarifying question" is coaching about the
            // conversation rather than a judgment of how it sounded.
            " hesitation ",
            " hesitations ",
            " hesitant ",
            " hesitantly ",
            " eye contact ",
            " body posture ",
            " your posture ",
            " their posture ",
            " his posture ",
            " her posture ",
            " body language ",
            " facial expression ",
            " facial expressions ",
            " voice tone ",
            " vocal tone ",
            " tone of voice ",
            " attractiveness ",
            " physical appearance ",
            " nervous ",
            " nervousness ",
            " nervously ",
            " anxious ",
            " anxiety ",
            " confident demeanor ",
            " lacked confidence ",
            " lack of confidence ",
            " personality ",
            " introvert ",
            " extrovert ",
            " charisma ",
            " appeared confident ",
            " appears confident ",
            " seemed confident ",
            " looked confident ",
            " sounded confident ",
            " come across as confident ",
            " comes across as confident ",
            " came across as confident ",
            " sound confident ",
            " sounds confident ",
            " seem confident ",
            " seems confident ",
            " appear confident ",
            " look confident ",
            " looks confident ",
            " sounding confident ",
            " looking confident ",
            " seeming confident ",
            " appearing confident ",
        ],
        before_speech: &[],
    },
    // The language a transcript came out in is the recognizer's output, not the
    // candidate's: a report that said "an irrelevant response in Mandarin"
    // failed an English speaker for it. Named languages only with "in", except
    // the two that name nothing but speech, and none of "another language" or
    // "switched languages", because a candidate changing C++ for Python is a
    // coding event. The cost is a technical sentence such as "input in Chinese
    // characters" on a text exercise, refused and repaired; matching only after
    // a speech word would miss "answered the question in Japanese". The
    // adjective, "a Japanese response", is refused only before a speech word,
    // where it names the transcript's language rather than an input's.
    JudgmentRule {
        reason: "the recognizer's language attributed to the candidate",
        improvements_only: false,
        phrases: &[
            " in chinese ",
            " in japanese ",
            " in korean ",
            " in hindi ",
            " in spanish ",
            " in french ",
            " in german ",
            " in portuguese ",
            " mandarin ",
            " cantonese ",
            " non english ",
            " foreign language ",
            " native language ",
            " broken english ",
            " english proficiency ",
            " english fluency ",
            " fluent english ",
            " language barrier ",
        ],
        before_speech: &[
            "chinese",
            "japanese",
            "korean",
            "hindi",
            "spanish",
            "french",
            "german",
            "portuguese",
        ],
    },
    // Improvements only: the summary may say evidence was limited by
    // transcription. After one misrecognized turn a report asked for
    // explanations "in English to avoid transcription ambiguity", which is what
    // this refuses. A neutral mention of transcription is left alone: a report
    // that suggested typing a key point as a code comment "when a transcription
    // gap occurs" was refused through both repairs, and an incomplete report
    // costs the candidate their verdict. Not "english" alone either: "describe
    // the approach in plain English" is ordinary advice, and the live prompt
    // asks the interviewer for it. "Avoid" and "prevent" are what turn a
    // mention into blame: a seeded report asked for answers "clearly
    // articulated to avoid transcription ambiguity".
    JudgmentRule {
        reason: "a speech-recognition gap is not the candidate's weakness",
        improvements_only: true,
        phrases: &[
            " avoid transcription ",
            " prevent transcription ",
            " in english ",
            " speak english ",
            " your english ",
            " spoken english ",
            " primary spoken language ",
            " speak your primary language ",
            " speak in your primary language ",
            " interview language ",
            " language of the interview ",
            " audible ",
            " audibly ",
            " speak more clearly ",
            " speak clearly ",
            " enunciate ",
            " enunciation ",
            " pronounce ",
            " pronunciation ",
        ],
        before_speech: &[],
    },
];

#[cfg(test)]
#[path = "../../../tests/unit/report_judgment_rules.rs"]
mod tests;
