// The candidate's own camera, shown to the candidate.
//
// The integrity video is deliberately invisible, so a candidate recording the
// interview with a screen recorder captured the code and Jim but never their
// own face. The only way to get it was to have the recorder open the camera as
// well, and a camera CodeTrial already holds is one a second application on
// Windows usually cannot open: the recorder then held the device first and the
// preflight failed with nothing to do about it. Painting the track CodeTrial
// already has lets the recorder capture the screen alone.
//
// A second element rather than the integrity one made visible: the sampler
// owns that element's `srcObject` and clears it on teardown, and the two
// lifetimes are not the same.

/// Shows `stream`'s camera in `video` and returns the undo. Hidden whenever
/// there is nothing honest to show: no camera, one that ended, or one another
/// application has taken, which stays `live` and freezes on its last frame.
export function attachSelfView(video, stream) {
  const track = stream?.getVideoTracks?.()[0];
  if (!video) return () => {};
  const release = () => {
    video.hidden = true;
    video.srcObject = null;
  };
  // Cleared, not just hidden: an element left holding an earlier stream would
  // come back with it the moment anything unhid it.
  if (!track || track.readyState === "ended") {
    release();
    return () => {};
  }

  const paint = () => {
    video.hidden = track.readyState !== "live" || track.muted;
  };
  video.muted = true;
  video.playsInline = true;
  video.srcObject = new MediaStream([track]);
  void video.play?.()?.catch?.(() => {});
  paint();

  track.addEventListener?.("mute", paint);
  track.addEventListener?.("unmute", paint);
  track.addEventListener?.("ended", release);
  return () => {
    track.removeEventListener?.("mute", paint);
    track.removeEventListener?.("unmute", paint);
    track.removeEventListener?.("ended", release);
    release();
  };
}
