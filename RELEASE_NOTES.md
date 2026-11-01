# Release notes

## v1.0.1

Changes on `main` since `origin/main` (`3b125ba`, AgentMesh v1.0).

### New

- **New Window from the Dock.** Right-click the AgentMesh icon in the Dock and choose **New Window**. Each window is one project, so it asks for a project folder first. If that project already has a window open, that window comes to the front instead.

### Fixed

- **No more ghost text when switching between windowed and full screen.** During the transition, a faded, scaled copy of the old window used to show through the glass behind the new one. The window now turns opaque for the length of the animation and gets its glass back the moment macOS reports the transition finished.
- **Sidebar resize no longer gets stuck.** A plain click on the sidebar's edge, without dragging, used to leave the edge following the pointer afterwards. The drag now always ends on release, including a release outside the window. Double-clicking the edge no longer resets the width to 280px.
- **Title bar in full screen.** Full screen hides the traffic lights, so the sidebar's title bar content now moves to the left edge instead of leaving an empty gap.

### Changed

- **Renamed to AgentMesh throughout.** The menus now say "Hide AgentMesh" and "Quit AgentMesh". The sidebar now says "Looking for agent…" and "Agent CLI not found" where it used to name Claude Code. Docs and the README use "agent CLI".
- **Binary renamed** from `claude-control-plane` to `agentmesh`. `scripts/bundle.sh` packages the new name; the app bundle is still `AgentMesh.app`.
- AgentMesh still runs `claude` from your login shell's `PATH`. Only names changed, not what gets launched.

### Upgrade notes

- **Preferences reset.** Preferences now live in `~/Library/Application Support/AgentMesh/prefs.json` instead of `.../ClaudeControlPlane/prefs.json`. Font size, sidebar open/width and automatic naming start from defaults. To keep yours, copy the old file over before first launch:

  ```sh
  mkdir -p ~/Library/Application\ Support/AgentMesh
  cp ~/Library/Application\ Support/ClaudeControlPlane/prefs.json ~/Library/Application\ Support/AgentMesh/
  ```

- **New log file.** Logs now go to `~/Library/Logs/AgentMesh.log` instead of `ClaudeControlPlane.log`.
- Session history is unaffected: it is read from `~/.claude` as before.

### Install

```sh
scripts/bundle.sh --install
```
