import { downloadText } from "./download.js";
import { peerApi } from "./peer-lib.js";

export function mountPeerArtifacts({
  nodes,
  sessionId,
  settings,
  members,
  onError,
}) {
  const base = `/api/peer/sessions/${encodeURIComponent(sessionId)}`;
  let stopped = false;
  let timer;
  let refreshing = false;
  nodes.panel.hidden = false;
  nodes.transcript.hidden = !settings.saveTranscript;
  nodes.withdrawTranscript.hidden = !settings.saveTranscript;
  nodes.audio.hidden = true;
  nodes.withdraw.hidden = true;
  nodes.transcriptNote.textContent = settings.saveTranscript
    ? "Your saved transcript is available only to you and your partner for 24 hours after practice ends."
    : "Transcript saving is off. Temporary transcript content is deleted after advice is produced.";
  nodes.audioNote.textContent = settings.saveAudio
    ? "Checking saved audio..."
    : "Audio saving was off for this session.";
  function renderAdvice(answer) {
    nodes.advice.replaceChildren();
    if (answer.status !== "ready") {
      nodes.advice.textContent =
        answer.status === "unavailable"
          ? "AI suggestions are unavailable for this session. Missing observation is never treated as a performance judgment."
          : "Preparing your post-session suggestions...";
      return;
    }
    if (!answer.advice?.length) {
      nodes.advice.textContent =
        "There was not enough observed conversation to give concrete suggestions.";
      return;
    }
    for (const advice of answer.advice) {
      const section = document.createElement("section");
      const title = document.createElement("h3");
      title.textContent = `Round ${Number(advice.roundIndex) + 1} · Your ${advice.role} practice`;
      section.append(title);
      for (const [key, label] of [
        ["strengths", "What worked"],
        ["improvements", "Try next time"],
      ]) {
        if (!advice[key]?.length) continue;
        const heading = document.createElement("h4");
        heading.textContent = label;
        const list = document.createElement("ul");
        for (const text of advice[key]) {
          const item = document.createElement("li");
          item.textContent = text;
          list.append(item);
        }
        section.append(heading, list);
      }
      const moments = (advice.evidenceRefs ?? [])
        .map((ref) => Number(ref?.segmentIndex))
        .filter((index) => Number.isInteger(index) && index >= 0);
      if (moments.length) {
        const heading = document.createElement("h4");
        heading.textContent = "Observed examples";
        const list = document.createElement("ul");
        for (const index of moments) {
          const item = document.createElement("li");
          item.textContent = `Grounded in conversation moment ${index + 1}.`;
          list.append(item);
        }
        section.append(heading, list);
      }
      nodes.advice.append(section);
    }
  }
  // Everything saved for a practice is removed 24 hours after it ends, so no
  // control that fetches or retries it is left.
  function expire() {
    nodes.advice.textContent =
      "The suggestions and saved files for this practice have expired.";
    nodes.audio.hidden = true;
    nodes.withdraw.hidden = true;
    nodes.transcript.hidden = true;
    nodes.withdrawTranscript.hidden = true;
    nodes.retry.hidden = true;
    nodes.audioNote.textContent = "";
    nodes.transcriptNote.textContent = "";
  }
  async function refresh() {
    if (stopped || refreshing) return;
    refreshing = true;
    let pending = false;
    try {
      const answer = await peerApi(`${base}/advice`);
      if (stopped) return;
      renderAdvice(answer);
      pending = ["processing", "observing"].includes(answer.status);
    } catch (error) {
      if (error.code === "not_found") {
        expire();
        refreshing = false;
        return;
      }
      nodes.advice.textContent =
        "AI suggestions could not be loaded. Try again.";
      onError?.(error);
    }
    if (settings.saveAudio && !stopped) {
      try {
        const audio = await peerApi(`${base}/audio/status`);
        if (stopped) return;
        nodes.audio.hidden = audio.status !== "ready";
        nodes.withdraw.hidden = audio.status !== "ready";
        nodes.audio.href = `${base}/audio`;
        nodes.audioNote.textContent =
          {
            recording: "Finalizing your saved audio...",
            ready:
              "Saved audio is available only to you and your partner for 24 hours after practice ends.",
            failed: "Audio could not be saved for this session.",
            deleted: "The saved audio has been deleted.",
            unavailable: "No saved audio is available for this session.",
          }[audio.status] ?? "Checking saved audio...";
        pending ||= audio.status === "recording";
      } catch (error) {
        nodes.audioNote.textContent =
          "Saved audio could not be loaded. Try again.";
        onError?.(error);
      }
    }
    refreshing = false;
    clearTimeout(timer);
    if (pending && !stopped) timer = setTimeout(() => void refresh(), 5000);
  }
  nodes.retry.addEventListener("click", () => void refresh());
  nodes.transcript.addEventListener("click", async () => {
    nodes.transcript.disabled = true;
    try {
      const answer = await peerApi(`${base}/transcript`);
      if (answer.status === "processing" || answer.status === "observing") {
        nodes.transcriptNote.textContent =
          "The transcript is still being finalized. Try again shortly.";
        return;
      }
      if (answer.status === "unavailable" && !answer.segments?.length) {
        nodes.transcript.hidden = true;
        nodes.withdrawTranscript.hidden = true;
        nodes.transcriptNote.textContent =
          "No transcript is available for this session.";
        return;
      }
      const text = (answer.segments ?? [])
        .map((segment) => {
          const member = members.find((member) => member.slot === segment.slot);
          const name = member
            ? `${member.nickname || member.login} (@${member.login})`
            : "Participant";
          return `${segment.roundIndex === null ? "Preparation" : `Round ${Number(segment.roundIndex) + 1}`} · ${name}\n${segment.text}`;
        })
        .join("\n\n");
      downloadText(
        text,
        `peer-${sessionId}-transcript.txt`,
        "text/plain;charset=utf-8",
      );
    } catch (error) {
      if (error.code === "not_found") {
        nodes.transcript.hidden = true;
        nodes.withdrawTranscript.hidden = true;
        nodes.transcriptNote.textContent =
          "This transcript is no longer available.";
        // The advice answers whether the whole practice expired or only the
        // transcript was withdrawn.
        await refresh();
        return;
      }
      nodes.transcriptNote.textContent =
        "The transcript could not be downloaded. Try again.";
      onError?.(error);
    } finally {
      nodes.transcript.disabled = false;
    }
  });
  nodes.withdrawTranscript.addEventListener("click", async () => {
    nodes.withdrawTranscript.disabled = true;
    try {
      await peerApi(`${base}/transcript`, {}, "DELETE");
      nodes.transcript.hidden = true;
      nodes.withdrawTranscript.hidden = true;
      nodes.transcriptNote.textContent =
        "The saved transcript has been deleted. This does not remove post-session suggestions.";
    } catch (error) {
      onError?.(error);
    } finally {
      nodes.withdrawTranscript.disabled = false;
    }
  });
  nodes.withdraw.addEventListener("click", async () => {
    nodes.withdraw.disabled = true;
    try {
      await peerApi(`${base}/audio`, {}, "DELETE");
      await refresh();
    } catch (error) {
      onError?.(error);
    } finally {
      nodes.withdraw.disabled = false;
    }
  });
  void refresh();
  return {
    stop() {
      stopped = true;
      clearTimeout(timer);
    },
  };
}
