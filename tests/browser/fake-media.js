// Stand-ins for the camera and microphone, shared by the tests that drive a
// real device pool. Importing this file installs the `MediaStream` the pool
// builds its own stream with, which node does not have.

export class FakeTrack {
  constructor(kind) {
    this.kind = kind;
    this.stopped = false;
    this.readyState = "live";
    this.enabled = true;
    this.muted = false;
  }
  stop() {
    this.stopped = true;
  }
}

export class FakeStream {
  constructor(tracks = []) {
    this.tracks = tracks;
  }
  getTracks() {
    return this.tracks;
  }
  addTrack(track) {
    this.tracks.push(track);
  }
  removeTrack(track) {
    this.tracks = this.tracks.filter((each) => each !== track);
  }
}

globalThis.MediaStream = FakeStream;

export const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
