# Google Meet tab-audio manual check

The browser tests read text. They cannot prove that Meet actually carries Jim's
voice to the interviewer, because that path runs entirely inside Chrome and
Meet. This is the procedure that does, and the table at the bottom is the
evidence.

Re-run this when the Meet mode UI changes, when the LiveKit client is
re-vendored, or before relying on the mode for a real interview.

## Environment

- Browser: Chrome stable (record the exact version in the results table).
- Two accounts, or one account plus a second device, so there is a real Meet
  participant on the receiving end.
- Headphones on the candidate machine. Without them the candidate microphone
  re-captures Jim from the speakers, and the interviewer hears him twice.

## Procedure

1. Start a CodeTrial interview and complete the media preflight (output,
   microphone, camera). Wait until Jim has spoken at least once, so the remote
   audio element exists.
2. Open "Present in Google Meet" in the sidebar. Confirm "Play Jim through"
   lists real device names, not "Output 1". Choose a non-default output and
   confirm Jim moves to it. This step is what catches a broken device list or a
   preference that fails to persist, so do not skip it when Jim is already
   audible.
3. In Meet, click Present now.
4. Choose **A tab**.
5. Select the CodeTrial tab.
6. Enable **Share tab audio** before clicking Share.
7. Confirm Meet's microphone is still the candidate microphone, not "None" and
   not a virtual device.
8. Record 10 seconds in Meet while Jim is speaking and the candidate says one
   sentence.

## Pass criteria

All five must hold:

- The recording contains Jim's voice.
- The recording contains the CodeTrial tab video.
- The recording contains the candidate's voice.
- Meet's selected microphone is still the candidate's physical microphone at
  the end of the run.
- The output device chosen in step 2 is still the one playing Jim, and it is
  still selected after a page reload.

Any failure is a fail row. Note which of the five broke.

## Privacy

The verification recording is deleted once the row below is filled in. Record
the filename and date, not the file. Do not commit the recording, and do not
use a real candidate's interview as the test subject.

## Results

| Date | Chrome version | Recording filename (deleted after check) | Result | Notes |
|---|---|---|---|---|
| | | | | |
