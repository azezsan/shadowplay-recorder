# ShadowPlay Recorder

A Windows tray app that records every Deadlock match with NVIDIA ShadowPlay. When a real match starts it starts a recording, when the match ends it stops and saves the video named after the match id (for example `81038409.mp4`), and it deletes the oldest recordings once the folder passes a size cap.

It needs Windows, an NVIDIA GPU with the NVIDIA App, and Deadlock on Steam.

> The app is being rewritten from a Python script into a desktop app. There is no installer yet. The last Python version is at commit `582043e`.

## How it works

- It follows Deadlock's console log to see a match start, the map load, and the match end. Bot, sandbox and custom maps are ignored.
- It sends ShadowPlay's own record command through the NVIDIA overlay, the same one the overlay's Record button sends. No keypress is simulated.
- It treats a recording as started only once the video file appears, and re-sends the start command until it does.
- It never sends a stop unless a recording exists, so it cannot start one by accident in the lobby.

## Security note

The app reaches the overlay through its debug port, which it opens by adding one line to the NVIDIA App's overlay config (the setup checklist's Fix button, behind one administrator prompt; the original is kept as `NVIDIA Overlay.json.bak`). The debug port listens only on loopback and has no authentication: any program running on the same PC can use it to run code in the overlay page. Other machines cannot reach it. This is accepted for a personal gaming PC.

## Development

```sh
pnpm install
pnpm dev     # run the app
pnpm check   # every formatter, linter and test
```

The app runs on macOS for UI work, where it stays idle because there is no game to detect. See `CLAUDE.md` for the layout and rules.
