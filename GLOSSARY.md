# ShadowPlay Recorder

A Windows tray app that records each Deadlock match with NVIDIA ShadowPlay and names the video after the match.

## Language

### Matches

**Match**:
One game of Deadlock, from its lobby being created to its lobby being destroyed, as reported by the console log.
_Avoid_: Game, round, session

**Match id**:
The number Deadlock gives a match. A recording is named after it.

**Console log**:
The text file Deadlock writes when launched with `-condebug`. It is the only source of match events.
_Avoid_: Game log, debug log

**Recordable map**:
A map that means a real match. Bot, sandbox and custom maps are not recordable, and a match on one is never recorded.

**Phase**:
What the recorder is doing right now: idle, game running, in match, starting, recording, or finishing.
_Avoid_: State, mode

### Recording

**Recording**:
The video file ShadowPlay writes for one match.
_Avoid_: Clip, capture, video

**Record command**:
ShadowPlay's own start or stop instruction, sent through the overlay. Never a simulated keypress.
_Avoid_: Hotkey, keypress

**Confirmed recording**:
A recording proven to have started because its file appeared in the recordings folder. The overlay's reply is not proof.

**Start window**:
The time after a recordable map loads during which the start command is re-sent until the recording is confirmed.

**Stop delay**:
How long recording continues after a match ends, to capture the post-match screen.

**Recordings folder**:
The folder ShadowPlay saves recordings to, and the folder the retention rule applies to.

**Retention rule**:
How the app decides when to delete the oldest recordings: either the size cap or fill the disk.
_Avoid_: Mode, policy

**Size cap**:
The retention rule that deletes the oldest recordings once the recordings folder grows past a chosen total size.
_Avoid_: Quota, limit

**Fill the disk**:
The retention rule that lets recordings use the whole disk holding the recordings folder, except the free-space reserve.

**Free-space reserve**:
The free space kept on the recordings folder's disk so the next recording fits. Under fill the disk, the oldest recordings are deleted to keep it.
_Avoid_: Headroom, buffer

### NVIDIA

**Overlay**:
The NVIDIA App's in-game overlay, the page that holds ShadowPlay's Record button.
_Avoid_: NVIDIA App (the overlay is one part of it), GeForce Experience

**Debug port**:
The loopback port the overlay opens once its config carries the debug switch. Record commands travel over it.

**Debug switch**:
The line in the overlay's config file that opens the debug port. An NVIDIA App update can remove it.

### App

**Setup checklist**:
The three conditions that must hold before a match can be recorded: the overlay is reachable, Deadlock writes its console log, and a recordings folder is set.

**Activity**:
The running list of what the recorder did, shown in the window and written to a log file.
_Avoid_: Logs, history
